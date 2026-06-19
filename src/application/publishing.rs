use crate::domain::*;
use crate::infrastructure::*;
use crate::support::*;
use serde_json::json;
use std::path::PathBuf;

pub(crate) fn cmd_plan() -> AppResult<serde_json::Value> {
    let resolved = resolve_config()?;
    let root = PathBuf::from(&resolved.root);
    let source = PathBuf::from(&resolved.source_abs);
    let documents = list_documents(&root, &source)?;
    validate_directory_index_conflicts(&resolved, &documents)?;
    validate_unique_slugs(&documents)?;
    let items = documents
        .into_iter()
        .map(|doc| PlanItem {
            path: doc.path,
            title: doc.title,
            action: "publish",
            reason: "status tracking is not implemented yet",
            confluence_url: None,
        })
        .collect::<Vec<_>>();

    Ok(ok(json!({
        "target": resolved.target,
        "count": items.len(),
        "items": items,
    })))
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
