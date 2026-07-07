use crate::domain::*;
use crate::infrastructure::*;
use crate::support::*;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;

/// Read-only aggregate of the bound source and its publish state: every
/// document fingerprinted next to what was last published. Shared by
/// `status` and `plan`; takes no lock, stages nothing, and performs no
/// remote calls. The write path (`publish`/`sync`) keeps its own
/// `prepare_publish_set`, which layers subset selection, the hard
/// missing-asset refusal, and staging on top of the same inputs.
pub(super) struct PublishStateView {
    pub(super) resolved: ResolvedConfig,
    pub(super) stage_root: PathBuf,
    pub(super) state_path: PathBuf,
    pub(super) documents: Vec<Document>,
    pub(super) snapshots: Vec<DocumentSnapshot>,
    pub(super) state: SyncState,
}

pub(super) fn load_publish_state_view() -> AppResult<PublishStateView> {
    let resolved = resolve_config()?;
    let root = PathBuf::from(&resolved.root);
    let source = PathBuf::from(&resolved.source_abs);
    let documents = list_documents(&root, &source)?;
    validate_directory_index_conflicts(&resolved, &documents)?;
    validate_unique_slugs(&documents)?;
    let hierarchy = build_hierarchy(&resolved, &documents, &documents)?;
    let snapshots = snapshot_hierarchy(&root, &hierarchy)?;
    let stage_root = publish_stage_root(&resolved)?;
    let identity = sync_state_identity(&resolved);
    let state_path = sync_state_path(&stage_root);
    let state = load_sync_state(&state_path, &identity)?;

    Ok(PublishStateView {
        resolved,
        stage_root,
        state_path,
        documents,
        snapshots,
        state,
    })
}

pub(crate) fn cmd_plan() -> AppResult<serde_json::Value> {
    let view = load_publish_state_view()?;
    let root = PathBuf::from(&view.resolved.root);
    let missing_assets: HashMap<String, Vec<String>> =
        missing_staged_asset_references(&root, &view.documents)?
            .into_iter()
            .collect();

    let (mut sync_items, _) =
        build_sync_plan(&view.snapshots, &view.state, true, &view.resolved.source);
    join_sync_items_with_typub_status(&view.stage_root, &mut sync_items)?;

    // A document that references assets the shared `_assets/` staging cannot
    // provide is blocked regardless of its publish-state verdict: publishing
    // it would fail locally, so no state-derived action applies.
    let items = sync_items
        .into_iter()
        .map(|item| match missing_assets.get(&item.path) {
            Some(missing) => PlanItem {
                path: item.path,
                title: item.title,
                action: "blocked".to_string(),
                reason: format!(
                    "references assets not present in _assets/: {}",
                    missing.join(", ")
                ),
                confluence_url: item.url,
            },
            None => PlanItem {
                path: item.path,
                title: item.title,
                action: item.action,
                reason: item.reason.unwrap_or_default(),
                confluence_url: item.url,
            },
        })
        .collect::<Vec<_>>();

    let publishable = items
        .iter()
        .filter(|item| item.action == "create" || item.action == "update")
        .count();

    Ok(ok(json!({
        "target": view.resolved.target,
        "state_file": display_path(&view.state_path),
        "count": items.len(),
        "publishable": publishable,
        "summary": plan_counts(&items),
        "items": items,
    })))
}

fn plan_counts(items: &[PlanItem]) -> serde_json::Value {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in items {
        *counts.entry(item.action.as_str()).or_default() += 1;
    }

    json!({
        "create": counts.get("create").copied().unwrap_or(0),
        "update": counts.get("update").copied().unwrap_or(0),
        "unchanged": counts.get("unchanged").copied().unwrap_or(0),
        "deleted": counts.get("deleted").copied().unwrap_or(0),
        "blocked": counts.get("blocked").copied().unwrap_or(0),
    })
}

pub(crate) fn cmd_publish(
    yes: bool,
    dry_run: bool,
    delay_ms: u64,
    concurrency: usize,
) -> AppResult<serde_json::Value> {
    if !dry_run && !yes {
        return Err(AppError::new(
            "CONFIRMATION_REQUIRED",
            "run `conpub publish --yes` to allow remote writes",
        ));
    }
    validate_publish_pacing(concurrency)?;

    let (prepared, _) = prepare_publish_set(&[])?;

    if dry_run {
        stage_dry_run_snapshots(&prepared)?;
        let items = dry_run_publish_items(&prepared.snapshots);

        return Ok(ok(json!({
            "dry_run": true,
            "target": &prepared.resolved.target,
            "stage_root": display_path(&prepared.stage_root),
            "typub_status_db": display_path(&typub_status_db_path(&prepared.stage_root)),
            "count": items.len(),
            "pacing": publish_pacing_json(delay_ms, concurrency),
            "items": items,
        })));
    }

    let _state_lock = lock_sync_state(&prepared.stage_root)?;
    let state = load_sync_state(&prepared.state_path, &prepared.identity)?;
    let items = publish_prepared_snapshots(&prepared, &prepared.snapshots, &state, delay_ms)?;
    let failed = items.iter().filter(|item| item.error.is_some()).count();
    update_sync_state_from_publish_results(
        &prepared.state_path,
        &prepared.identity,
        &prepared.snapshots,
        &items,
    )?;

    Ok(ok(json!({
        "dry_run": false,
        "target": &prepared.resolved.target,
        "stage_root": display_path(&prepared.stage_root),
        "typub_status_db": display_path(&typub_status_db_path(&prepared.stage_root)),
        "count": items.len(),
        "failed": failed,
        "pacing": publish_pacing_json(delay_ms, concurrency),
        "items": items,
    })))
}

pub(crate) fn cmd_sync(
    paths: Vec<String>,
    yes: bool,
    dry_run: bool,
    delay_ms: u64,
    concurrency: usize,
    archive_deleted: bool,
) -> AppResult<serde_json::Value> {
    if !dry_run && !yes {
        return Err(AppError::new(
            "CONFIRMATION_REQUIRED",
            "run `conpub sync --dry-run` to inspect changes or `conpub sync --yes` to allow remote writes",
        ));
    }
    validate_publish_pacing(concurrency)?;

    let (prepared, subset) = prepare_publish_set(&paths)?;
    let _state_lock = if dry_run {
        None
    } else {
        Some(lock_sync_state(&prepared.stage_root)?)
    };
    let mut state = load_sync_state(&prepared.state_path, &prepared.identity)?;
    let (mut items, publish_snapshots) = build_sync_plan(
        &prepared.snapshots,
        &state,
        !subset,
        &prepared.resolved.source,
    );
    join_sync_items_with_typub_status(&prepared.stage_root, &mut items)?;
    mark_superseded_deleted(&mut items);
    apply_archive_deleted_plan(&mut items, archive_deleted);

    if dry_run {
        return Ok(ok(json!({
            "dry_run": true,
            "subset": subset,
            "archive_deleted": archive_deleted,
            "target": &prepared.resolved.target,
            "stage_root": display_path(&prepared.stage_root),
            "state_file": display_path(&prepared.state_path),
            "typub_status_db": display_path(&typub_status_db_path(&prepared.stage_root)),
            "count": items.len(),
            "publishable": publish_snapshots.len(),
            "pacing": publish_pacing_json(delay_ms, concurrency),
            "summary": sync_counts(&items),
            "items": items,
        })));
    }

    if !publish_snapshots.is_empty() {
        let publish_results =
            publish_prepared_snapshots(&prepared, &publish_snapshots, &state, delay_ms)?;
        merge_publish_results_into_sync_items(&mut items, &publish_results);
        update_sync_state_from_sync_results(&mut state, &prepared.snapshots, &items);
    }

    // Re-mark after publish results land: a create that adopted an existing
    // page by title only now carries its page id, and the matching deleted
    // entry must never reach the archive call below.
    mark_superseded_deleted(&mut items);
    remove_superseded_deleted_from_state(&mut state, &items);

    if archive_deleted {
        let archive_submitted = runtime_archive_deleted(&prepared.resolved, &mut items)?;
        if archive_submitted {
            remove_archived_deleted_from_state(&mut state, &items);
        }
    }
    write_sync_state(&prepared.state_path, &state)?;

    let failed = items.iter().filter(|item| item.status == "failed").count();

    Ok(ok(json!({
        "dry_run": false,
        "subset": subset,
        "archive_deleted": archive_deleted,
        "target": &prepared.resolved.target,
        "stage_root": display_path(&prepared.stage_root),
        "state_file": display_path(&prepared.state_path),
        "typub_status_db": display_path(&typub_status_db_path(&prepared.stage_root)),
        "count": items.len(),
        "publishable": publish_snapshots.len(),
        "failed": failed,
        "pacing": publish_pacing_json(delay_ms, concurrency),
        "summary": sync_counts(&items),
        "items": items,
    })))
}

/// Reconcile deleted state entries. Default is bookkeeping-only: entries are
/// dropped from the local state and the remote pages are left in place.
/// `--archive` archives the pages first; `--delete` deletes them permanently.
/// Superseded entries (page id owned by a live document after an adoption)
/// are always dropped without any remote action, whatever flags are set.
pub(crate) fn cmd_prune(yes: bool, archive: bool, delete: bool) -> AppResult<serde_json::Value> {
    if archive && delete {
        return Err(AppError::new(
            "PRUNE_CONFLICTING_FLAGS",
            "--archive and --delete are mutually exclusive",
        ));
    }

    let (prepared, _subset) = prepare_publish_set(&[])?;
    let _state_lock = if yes {
        Some(lock_sync_state(&prepared.stage_root)?)
    } else {
        None
    };
    let mut state = load_sync_state(&prepared.state_path, &prepared.identity)?;
    let (mut items, _) =
        build_sync_plan(&prepared.snapshots, &state, true, &prepared.resolved.source);
    join_sync_items_with_typub_status(&prepared.stage_root, &mut items)?;
    mark_superseded_deleted(&mut items);
    let mut items = items
        .into_iter()
        .filter(|item| item.action == "deleted")
        .collect::<Vec<_>>();

    for item in &mut items {
        if item.status == "superseded" {
            continue;
        }
        let (status, reason) = match (&item.platform_id, archive, delete) {
            (Some(_), true, _) => (
                "pending_archive",
                "would archive the Confluence page, then drop the state entry",
            ),
            (Some(_), _, true) => (
                "pending_delete",
                "would delete the Confluence page permanently, then drop the state entry",
            ),
            (Some(_), false, false) => (
                "pending_prune",
                "would drop the state entry; the Confluence page is left in place",
            ),
            (None, ..) => (
                "pending_prune",
                "no Confluence page id is known; would drop the state entry",
            ),
        };
        item.status = status.to_string();
        item.reason = Some(reason.to_string());
    }

    if !yes {
        return Ok(ok(json!({
            "dry_run": true,
            "archive": archive,
            "delete": delete,
            "target": &prepared.resolved.target,
            "state_file": display_path(&prepared.state_path),
            "count": items.len(),
            "items": items,
        })));
    }

    if archive {
        runtime_archive_deleted(&prepared.resolved, &mut items)?;
    } else if delete {
        runtime_delete_deleted(&prepared.resolved, &mut items)?;
    }

    let mut pruned = 0_usize;
    for item in &mut items {
        let drop_entry = matches!(
            item.status.as_str(),
            "superseded" | "archived" | "deleted_remote" | "pending_prune"
        );
        if drop_entry {
            state.documents.remove(&item.path);
            pruned += 1;
            if item.status == "pending_prune" {
                item.status = "pruned".to_string();
                item.reason = Some("state entry dropped".to_string());
            }
        }
    }
    write_sync_state(&prepared.state_path, &state)?;

    let failed = items.iter().filter(|item| item.status == "failed").count();

    Ok(ok(json!({
        "dry_run": false,
        "archive": archive,
        "delete": delete,
        "target": &prepared.resolved.target,
        "state_file": display_path(&prepared.state_path),
        "count": items.len(),
        "pruned": pruned,
        "failed": failed,
        "items": items,
    })))
}

struct PreparedPublishSet {
    resolved: ResolvedConfig,
    stage_root: PathBuf,
    identity: SyncStateIdentity,
    state_path: PathBuf,
    snapshots: Vec<DocumentSnapshot>,
}

fn prepare_publish_set(paths: &[String]) -> AppResult<(PreparedPublishSet, bool)> {
    let resolved = resolve_config()?;
    let root = PathBuf::from(&resolved.root);
    let source = PathBuf::from(&resolved.source_abs);
    let subset = !paths.is_empty();
    let all_documents = list_documents(&root, &source)?;
    validate_directory_index_conflicts(&resolved, &all_documents)?;
    validate_unique_slugs(&all_documents)?;

    let selected_documents = if subset {
        resolve_document_subset(&root, &source, paths)?
    } else {
        all_documents.clone()
    };
    let hierarchy = build_hierarchy(&resolved, &selected_documents, &all_documents)?;

    // Refuse the publish set locally when any document to be published
    // references an asset the shared `_assets/` staging cannot provide —
    // otherwise the failure surfaces half-way remote, after page creation.
    let publish_documents: Vec<Document> = hierarchy
        .iter()
        .map(|entry| entry.document.clone())
        .collect();
    let missing = missing_staged_asset_references(&root, &publish_documents)?;
    if !missing.is_empty() {
        let detail = missing
            .iter()
            .map(|(path, refs)| format!("{path}: {}", refs.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(AppError::new(
            "ASSET_MISSING",
            format!(
                "documents reference assets not present in _assets/ (place files under <root>/_assets/ and reference them as assets/<name>): {detail}"
            ),
        ));
    }

    let stage_root = publish_stage_root(&resolved)?;
    let identity = sync_state_identity(&resolved);
    let state_path = sync_state_path(&stage_root);
    let snapshots = snapshot_hierarchy(&root, &hierarchy)?;

    Ok((
        PreparedPublishSet {
            resolved,
            stage_root,
            identity,
            state_path,
            snapshots,
        },
        subset,
    ))
}

fn publish_prepared_snapshots(
    prepared: &PreparedPublishSet,
    snapshots: &[DocumentSnapshot],
    state: &SyncState,
    delay_ms: u64,
) -> AppResult<Vec<PublishItemResult>> {
    let typub_config = build_typub_config(&prepared.resolved, &prepared.stage_root)?;
    let runtime = build_async_runtime()?;
    runtime.block_on(publish_snapshots_with_hierarchy(
        &typub_config,
        &prepared.resolved,
        &prepared.stage_root,
        snapshots,
        state,
        delay_ms,
    ))
}

fn stage_dry_run_snapshots(prepared: &PreparedPublishSet) -> AppResult<()> {
    for snapshot in &prepared.snapshots {
        stage_document_files(&prepared.resolved, &snapshot.document, &prepared.stage_root)?;
    }

    Ok(())
}

fn dry_run_publish_items(snapshots: &[DocumentSnapshot]) -> Vec<PublishItemResult> {
    snapshots
        .iter()
        .map(|item| PublishItemResult {
            path: item.document.path.clone(),
            title: item.document.title.clone(),
            slug: item.slug.clone(),
            parent_path: item.parent_path.clone(),
            parent_id: None,
            status: "dry_run".to_string(),
            url: None,
            platform_id: None,
            error: None,
        })
        .collect()
}
