use crate::domain::*;
use crate::support::*;
use fs2::FileExt;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SYNC_STATE_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct SyncStateLock {
    pub(crate) file: File,
}

impl Drop for SyncStateLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub(crate) fn sync_state_path(stage_root: &Path) -> PathBuf {
    stage_root.join(SYNC_STATE_FILE)
}

pub(crate) fn sync_state_lock_path(stage_root: &Path) -> PathBuf {
    stage_root.join("sync-state.lock")
}

pub(crate) fn lock_sync_state(stage_root: &Path) -> AppResult<SyncStateLock> {
    fs::create_dir_all(stage_root).map_err(|err| {
        AppError::new(
            "STATE_LOCK_ERROR",
            format!("failed to create {}: {err}", stage_root.display()),
        )
    })?;
    let path = sync_state_lock_path(stage_root);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|err| {
            AppError::new(
                "STATE_LOCK_ERROR",
                format!("failed to open {}: {err}", path.display()),
            )
        })?;
    file.lock_exclusive().map_err(|err| {
        AppError::new(
            "STATE_LOCK_ERROR",
            format!("failed to lock {}: {err}", path.display()),
        )
    })?;

    Ok(SyncStateLock { file })
}

pub(crate) fn sync_state_identity(resolved: &ResolvedConfig) -> SyncStateIdentity {
    SyncStateIdentity {
        root: resolved.root.clone(),
        source: ".".to_string(),
        base_url: resolved
            .target
            .base_url
            .as_deref()
            .map(normalize_confluence_base_url),
        space: resolved.target.space.clone(),
        parent_id: resolved.target.parent_id.clone(),
    }
}

pub(crate) fn load_sync_state(path: &Path, identity: &SyncStateIdentity) -> AppResult<SyncState> {
    if !path.exists() {
        return Ok(SyncState::new(identity.clone()));
    }

    let text = fs::read_to_string(path).map_err(|err| {
        AppError::new(
            "STATE_READ_ERROR",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let mut state: SyncState = serde_json::from_str(&text).map_err(|err| {
        AppError::new(
            "STATE_PARSE_ERROR",
            format!("failed to parse {}: {err}", path.display()),
        )
    })?;

    if state.version != SYNC_STATE_VERSION {
        return Err(AppError::new(
            "STATE_VERSION_UNSUPPORTED",
            format!(
                "unsupported sync state version {} in {}",
                state.version,
                path.display()
            ),
        ));
    }

    if let Some(existing) = &state.identity {
        if existing != identity {
            return Err(AppError::new(
                "STATE_TARGET_MISMATCH",
                format!(
                    "sync state target does not match current binding: {}",
                    path.display()
                ),
            ));
        }
    } else {
        state.identity = Some(identity.clone());
    }

    Ok(state)
}

pub(crate) fn update_sync_state_from_publish_results(
    state_path: &Path,
    identity: &SyncStateIdentity,
    snapshots: &[DocumentSnapshot],
    publish_results: &[PublishItemResult],
) -> AppResult<()> {
    let mut state = load_sync_state(state_path, identity)?;
    let snapshot_by_path = snapshots
        .iter()
        .map(|snapshot| (snapshot.document.path.as_str(), snapshot))
        .collect::<HashMap<_, _>>();
    let mut changed = false;

    for result in publish_results {
        if result.status != "published" || result.error.is_some() {
            continue;
        }

        if let Some(snapshot) = snapshot_by_path.get(result.path.as_str()) {
            state.documents.insert(
                result.path.clone(),
                SyncStateDocument {
                    fingerprint: snapshot.fingerprint.clone(),
                    title: snapshot.document.title.clone(),
                    slug: snapshot.slug.clone(),
                    parent_path: snapshot.parent_path.clone(),
                    synced_at: now_unix_seconds(),
                },
            );
            changed = true;
        }
    }

    if changed {
        write_sync_state(state_path, &state)?;
    }

    Ok(())
}

pub(crate) fn write_sync_state(path: &Path, state: &SyncState) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::new(
                "STATE_WRITE_ERROR",
                format!("failed to create {}: {err}", parent.display()),
            )
        })?;
    }

    let text = serde_json::to_string_pretty(state)
        .map_err(|err| AppError::new("STATE_ENCODE_ERROR", err.to_string()))?;
    let (tmp_path, mut file) = create_sync_state_temp_file(path)?;
    if let Err(err) = write_sync_state_temp_file(&mut file, &tmp_path, &text) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    drop(file);

    fs::rename(&tmp_path, path).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        AppError::new(
            "STATE_WRITE_ERROR",
            format!(
                "failed to replace {} with {}: {err}",
                path.display(),
                tmp_path.display()
            ),
        )
    })?;

    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }

    Ok(())
}

fn create_sync_state_temp_file(path: &Path) -> AppResult<(PathBuf, File)> {
    for attempt in 0..16 {
        let tmp_path = sync_state_temp_path(path, attempt);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
        {
            Ok(file) => return Ok((tmp_path, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(AppError::new(
                    "STATE_WRITE_ERROR",
                    format!("failed to create {}: {err}", tmp_path.display()),
                ));
            }
        }
    }

    Err(AppError::new(
        "STATE_WRITE_ERROR",
        format!(
            "failed to create a unique temporary file for {}",
            path.display()
        ),
    ))
}

fn write_sync_state_temp_file(file: &mut File, tmp_path: &Path, text: &str) -> AppResult<()> {
    file.write_all(text.as_bytes()).map_err(|err| {
        AppError::new(
            "STATE_WRITE_ERROR",
            format!("failed to write {}: {err}", tmp_path.display()),
        )
    })?;
    file.write_all(b"\n").map_err(|err| {
        AppError::new(
            "STATE_WRITE_ERROR",
            format!("failed to write {}: {err}", tmp_path.display()),
        )
    })?;
    file.sync_all().map_err(|err| {
        AppError::new(
            "STATE_WRITE_ERROR",
            format!("failed to sync {}: {err}", tmp_path.display()),
        )
    })
}

fn sync_state_temp_path(path: &Path, attempt: u32) -> PathBuf {
    let id = NEXT_SYNC_STATE_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!(
        "json.tmp-{}-{}-{id}-{attempt}",
        std::process::id(),
        unix_nanos()
    ))
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_state_temp_paths_are_unique_within_process() {
        let path = Path::new("/tmp/sync-state.json");

        let first = sync_state_temp_path(path, 0);
        let second = sync_state_temp_path(path, 0);

        assert_ne!(first, second);
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".tmp-")
        );
    }

    #[test]
    fn publish_state_update_does_not_persist_remote_publish_fields() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_path = temp.path().join("sync-state.json");
        let identity = SyncStateIdentity {
            root: "/kb".to_string(),
            source: "projects/cuda-agent".to_string(),
            base_url: Some("https://example.atlassian.net".to_string()),
            space: "GPU".to_string(),
            parent_id: "123456789".to_string(),
        };
        let snapshots = vec![DocumentSnapshot {
            document: Document {
                path: "projects/cuda-agent/notes.typ".to_string(),
                title: "Notes".to_string(),
                extension: "typ".to_string(),
                tags: vec!["inferlab".to_string()],
            },
            slug: "projects-cuda-agent-notes".to_string(),
            fingerprint: "fingerprint-v1".to_string(),
            parent_path: None,
            hierarchy_order: 0,
        }];
        let publish_results = vec![PublishItemResult {
            path: "projects/cuda-agent/notes.typ".to_string(),
            title: "Notes".to_string(),
            tags: vec!["inferlab".to_string()],
            slug: "projects-cuda-agent-notes".to_string(),
            parent_path: None,
            parent_id: Some("123456789".to_string()),
            status: "published".to_string(),
            url: Some("https://example.atlassian.net/wiki/spaces/GPU/pages/42/Notes".to_string()),
            platform_id: Some("42".to_string()),
            error: None,
        }];

        update_sync_state_from_publish_results(
            &state_path,
            &identity,
            &snapshots,
            &publish_results,
        )
        .expect("update state");

        let state = load_sync_state(&state_path, &identity).expect("load state");
        let entry = state
            .documents
            .get("projects/cuda-agent/notes.typ")
            .expect("state entry");
        assert_eq!(entry.slug, "projects-cuda-agent-notes");
        assert_eq!(entry.parent_path, None);
    }
}
