use super::models::*;
use crate::support::now_unix_seconds;
use serde_json::json;
use std::collections::{HashMap, HashSet};

pub(crate) fn build_sync_plan(
    snapshots: &[DocumentSnapshot],
    state: &SyncState,
    include_deleted: bool,
    source: &str,
) -> (Vec<SyncItemResult>, Vec<DocumentSnapshot>) {
    let mut items = Vec::new();
    let mut publish_snapshots = Vec::new();
    let mut current_paths = HashSet::new();

    for snapshot in snapshots {
        current_paths.insert(snapshot.document.path.clone());
        let previous = state.documents.get(&snapshot.document.path);
        let previous_fingerprint = previous.map(|entry| entry.fingerprint.clone());
        let (action, status, reason) = match previous {
            None => ("create", "pending", "not present in local publish state"),
            Some(entry) if entry.parent_path != snapshot.parent_path => (
                "update",
                "pending",
                "parent path differs from local publish state",
            ),
            Some(entry) if entry.fingerprint == snapshot.fingerprint => (
                "unchanged",
                "skipped",
                "fingerprint matches local publish state",
            ),
            Some(_) => (
                "update",
                "pending",
                "fingerprint differs from local publish state",
            ),
        };

        if action == "create" || action == "update" {
            publish_snapshots.push(snapshot.clone());
        }

        items.push(SyncItemResult {
            path: snapshot.document.path.clone(),
            title: snapshot.document.title.clone(),
            tags: snapshot.document.tags.clone(),
            slug: snapshot.slug.clone(),
            parent_path: snapshot.parent_path.clone(),
            parent_id: None,
            action: action.to_string(),
            status: status.to_string(),
            fingerprint: Some(snapshot.fingerprint.clone()),
            previous_fingerprint,
            url: None,
            platform_id: None,
            archive_task_id: None,
            error: None,
            reason: Some(reason.to_string()),
        });
    }

    if include_deleted {
        let mut deleted = state
            .documents
            .iter()
            .filter(|(path, _)| !current_paths.contains(*path) && path_is_in_source(path, source))
            .map(|(path, entry)| SyncItemResult {
                path: path.clone(),
                title: entry.title.clone(),
                tags: Vec::new(),
                slug: entry.slug.clone(),
                parent_path: entry.parent_path.clone(),
                parent_id: None,
                action: "deleted".to_string(),
                status: "skipped".to_string(),
                fingerprint: None,
                previous_fingerprint: Some(entry.fingerprint.clone()),
                url: None,
                platform_id: None,
                archive_task_id: None,
                error: None,
                reason: Some(
                    "local file is missing; run `conpub prune` to reconcile \
                     (optionally --archive or --delete the remote page)"
                        .to_string(),
                ),
            })
            .collect::<Vec<_>>();
        deleted.sort_by(|a, b| a.path.cmp(&b.path));
        items.extend(deleted);
    }

    (items, publish_snapshots)
}

fn path_is_in_source(path: &str, source: &str) -> bool {
    if source.is_empty() || source == "." {
        return true;
    }

    path == source
        || path
            .strip_prefix(source)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(crate) fn merge_publish_results_into_sync_items(
    items: &mut [SyncItemResult],
    publish_results: &[PublishItemResult],
) {
    let by_path = publish_results
        .iter()
        .map(|result| (result.path.as_str(), result))
        .collect::<HashMap<_, _>>();

    for item in items {
        if item.action != "create" && item.action != "update" {
            continue;
        }

        if let Some(result) = by_path.get(item.path.as_str()) {
            item.status = result.status.clone();
            item.parent_id = result.parent_id.clone();
            item.url = result.url.clone();
            item.platform_id = result.platform_id.clone();
            item.error = result.error.clone();
            item.reason = None;
        }
    }
}

/// Mark deleted entries whose Confluence page id is owned by a live document.
///
/// After a local move, provision adopts the remote page by title under the
/// new path, so the old path's deleted entry points at a page that is alive
/// under the new path; archiving or deleting through that entry would take
/// down the adopted page. Such entries are bookkeeping residue: they are
/// dropped from state without any remote action.
pub(crate) fn mark_superseded_deleted(items: &mut [SyncItemResult]) {
    let live_ids = items
        .iter()
        .filter(|item| item.action != "deleted")
        .filter_map(|item| item.platform_id.clone())
        .collect::<HashSet<_>>();

    for item in items {
        if item.action == "deleted"
            && item
                .platform_id
                .as_ref()
                .is_some_and(|id| live_ids.contains(id))
        {
            item.status = "superseded".to_string();
            item.reason = Some(
                "page id is owned by a live document (adopted after a move); \
                 the state entry is dropped without remote action"
                    .to_string(),
            );
        }
    }
}

pub(crate) fn remove_superseded_deleted_from_state(
    state: &mut SyncState,
    items: &[SyncItemResult],
) {
    for item in items {
        if item.action == "deleted" && item.status == "superseded" {
            state.documents.remove(&item.path);
        }
    }
}

pub(crate) fn apply_archive_deleted_plan(items: &mut [SyncItemResult], archive_deleted: bool) {
    if !archive_deleted {
        return;
    }

    for item in items {
        if item.action != "deleted" || item.status == "superseded" {
            continue;
        }

        if item.platform_id.is_some() {
            item.status = "pending_archive".to_string();
            item.reason = Some("would archive Confluence page from typub status".to_string());
        } else {
            item.status = "skipped".to_string();
            item.reason = Some("cannot archive deleted page without a Confluence ID".to_string());
        }
    }
}

pub(crate) fn remove_archived_deleted_from_state(state: &mut SyncState, items: &[SyncItemResult]) {
    for item in items {
        if item.action == "deleted" && item.status == "archived" {
            state.documents.remove(&item.path);
        }
    }
}

pub(crate) fn update_sync_state_from_sync_results(
    state: &mut SyncState,
    snapshots: &[DocumentSnapshot],
    items: &[SyncItemResult],
) {
    let snapshot_by_path = snapshots
        .iter()
        .map(|snapshot| (snapshot.document.path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();

    for item in items {
        if item.status != "published" {
            continue;
        }

        if let Some(snapshot) = snapshot_by_path.get(item.path.as_str()) {
            state.documents.insert(
                item.path.clone(),
                SyncStateDocument {
                    fingerprint: snapshot.fingerprint.clone(),
                    title: snapshot.document.title.clone(),
                    slug: snapshot.slug.clone(),
                    parent_path: snapshot.parent_path.clone(),
                    synced_at: now_unix_seconds(),
                },
            );
        }
    }
}

pub(crate) fn sync_counts(items: &[SyncItemResult]) -> serde_json::Value {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for item in items {
        *counts.entry(item.action.as_str()).or_default() += 1;
    }

    let superseded = items
        .iter()
        .filter(|item| item.status == "superseded")
        .count();

    json!({
        "create": counts.get("create").copied().unwrap_or(0),
        "update": counts.get("update").copied().unwrap_or(0),
        "unchanged": counts.get("unchanged").copied().unwrap_or(0),
        "deleted": counts.get("deleted").copied().unwrap_or(0),
        "superseded": superseded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_state_update_does_not_persist_remote_publish_fields() {
        let snapshots = vec![DocumentSnapshot {
            document: Document {
                path: "projects/cuda-agent/notes.typ".to_string(),
                title: "Notes".to_string(),
                extension: "typ".to_string(),
                tags: vec!["inferlab".to_string()],
            },
            slug: "projects-cuda-agent-notes".to_string(),
            fingerprint: "fingerprint-v1".to_string(),
            parent_path: Some("projects/cuda-agent/_index.md".to_string()),
            hierarchy_order: 1,
        }];
        let items = vec![SyncItemResult {
            path: "projects/cuda-agent/notes.typ".to_string(),
            title: "Notes".to_string(),
            tags: vec!["inferlab".to_string()],
            slug: "projects-cuda-agent-notes".to_string(),
            parent_path: Some("projects/cuda-agent/_index.md".to_string()),
            parent_id: Some("parent-42".to_string()),
            action: "update".to_string(),
            status: "published".to_string(),
            fingerprint: Some("fingerprint-v1".to_string()),
            previous_fingerprint: Some("fingerprint-v0".to_string()),
            url: Some("https://example.atlassian.net/wiki/spaces/GPU/pages/42/Notes".to_string()),
            platform_id: Some("42".to_string()),
            archive_task_id: None,
            error: None,
            reason: None,
        }];
        let mut state = SyncState::new(SyncStateIdentity {
            root: "/kb".to_string(),
            source: "projects/cuda-agent".to_string(),
            base_url: Some("https://example.atlassian.net".to_string()),
            space: "GPU".to_string(),
            parent_id: "123456789".to_string(),
        });

        update_sync_state_from_sync_results(&mut state, &snapshots, &items);

        let entry = state
            .documents
            .get("projects/cuda-agent/notes.typ")
            .expect("state entry");
        assert_eq!(entry.fingerprint, "fingerprint-v1");
        assert_eq!(
            entry.parent_path.as_deref(),
            Some("projects/cuda-agent/_index.md")
        );
    }

    fn item(path: &str, action: &str, platform_id: Option<&str>) -> SyncItemResult {
        SyncItemResult {
            path: path.to_string(),
            title: "Notes".to_string(),
            tags: Vec::new(),
            slug: path.replace(['/', '.'], "-"),
            parent_path: None,
            parent_id: None,
            action: action.to_string(),
            status: "skipped".to_string(),
            fingerprint: None,
            previous_fingerprint: None,
            url: None,
            platform_id: platform_id.map(str::to_string),
            archive_task_id: None,
            error: None,
            reason: None,
        }
    }

    #[test]
    fn superseded_marks_only_deleted_entries_whose_id_is_owned_by_a_live_item() {
        let mut items = vec![
            item("a/new.md", "update", Some("42")),
            item("a/old.md", "deleted", Some("42")),
            item("a/gone.md", "deleted", Some("43")),
            item("a/unknown.md", "deleted", None),
        ];

        mark_superseded_deleted(&mut items);

        assert_eq!(items[1].status, "superseded");
        assert_eq!(items[2].status, "skipped");
        assert_eq!(items[3].status, "skipped");
    }

    #[test]
    fn archive_plan_never_targets_superseded_entries() {
        let mut items = vec![
            item("a/new.md", "update", Some("42")),
            item("a/old.md", "deleted", Some("42")),
            item("a/gone.md", "deleted", Some("43")),
        ];
        mark_superseded_deleted(&mut items);

        apply_archive_deleted_plan(&mut items, true);

        assert_eq!(items[1].status, "superseded");
        assert_eq!(items[2].status, "pending_archive");
    }

    #[test]
    fn superseded_entries_are_dropped_from_state() {
        let mut items = vec![
            item("a/new.md", "update", Some("42")),
            item("a/old.md", "deleted", Some("42")),
        ];
        mark_superseded_deleted(&mut items);
        let mut state = SyncState::new(SyncStateIdentity {
            root: "/kb".to_string(),
            source: "a".to_string(),
            base_url: None,
            space: "GPU".to_string(),
            parent_id: "1".to_string(),
        });
        state.documents.insert(
            "a/old.md".to_string(),
            SyncStateDocument {
                fingerprint: "f0".to_string(),
                title: "Notes".to_string(),
                slug: "a-old-md".to_string(),
                parent_path: None,
                synced_at: 1,
            },
        );

        remove_superseded_deleted_from_state(&mut state, &items);

        assert!(state.documents.is_empty());
    }
}
