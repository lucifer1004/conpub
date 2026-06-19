use crate::domain::SyncItemResult;
use crate::support::*;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use typub_config::Config as TypubConfig;
use typub_engine::PublishContext;
use typub_storage::StatusTracker;

const CONFLUENCE_PLATFORM: &str = "confluence";

static TYPUB_STATUS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn typub_status_db_path(stage_root: &Path) -> PathBuf {
    stage_root.join(".typub").join("status.db")
}

pub(crate) fn new_publish_context_with_stage_status(
    config: &TypubConfig,
    stage_root: &Path,
) -> AppResult<PublishContext> {
    with_typub_status_lock(|| new_publish_context_with_stage_status_locked(config, stage_root))
}

pub(crate) fn join_sync_items_with_typub_status(
    stage_root: &Path,
    items: &mut [SyncItemResult],
) -> AppResult<()> {
    let Some(status) = load_existing_typub_status(stage_root)? else {
        return Ok(());
    };

    for item in items {
        let mut has_remote_status = false;
        if let Some(platform_id) = get_platform_id(&status, &item.slug)? {
            item.platform_id = Some(platform_id);
            has_remote_status = true;
        }
        if let Some(url) = get_published_url(&status, &item.slug)? {
            item.url = Some(url);
            has_remote_status = true;
        }
        if item.action == "create" && has_remote_status {
            item.action = "update".to_string();
            item.reason =
                Some("present in typub status but missing from local publish state".to_string());
        }
    }

    Ok(())
}

pub(crate) fn get_platform_id(status: &StatusTracker, slug: &str) -> AppResult<Option<String>> {
    status
        .get_platform_id(slug, CONFLUENCE_PLATFORM)
        .map_err(|err| AppError::new("PUBLISH_STATE_ERROR", err.to_string()))
}

fn get_published_url(status: &StatusTracker, slug: &str) -> AppResult<Option<String>> {
    status
        .get_published_url(slug, CONFLUENCE_PLATFORM)
        .map_err(|err| AppError::new("PUBLISH_STATE_ERROR", err.to_string()))
}

fn load_existing_typub_status(stage_root: &Path) -> AppResult<Option<StatusTracker>> {
    if !typub_status_db_path(stage_root).exists() {
        return Ok(None);
    }

    with_typub_status_lock(|| {
        with_typub_stage_workdir(stage_root, || {
            StatusTracker::load(stage_root)
                .map(Some)
                .map_err(|err| AppError::new("PUBLISH_STATE_ERROR", err.to_string()))
        })
    })
}

fn new_publish_context_with_stage_status_locked(
    config: &TypubConfig,
    stage_root: &Path,
) -> AppResult<PublishContext> {
    with_typub_stage_workdir(stage_root, || {
        PublishContext::new_with_root(config, stage_root)
            .map_err(|err| AppError::new("PUBLISH_STATE_ERROR", err.to_string()))
    })
}

fn with_typub_status_lock<T>(op: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
    let lock = TYPUB_STATUS_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().map_err(|_| {
        AppError::new(
            "PUBLISH_STATE_ERROR",
            "typub status working-directory lock is poisoned",
        )
    })?;
    op()
}

fn with_typub_stage_workdir<T>(
    stage_root: &Path,
    op: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    let _workdir = CurrentDirGuard::enter(stage_root)?;
    op()
}

struct CurrentDirGuard {
    previous: PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &Path) -> AppResult<Self> {
        fs::create_dir_all(path).map_err(|err| {
            AppError::new(
                "PUBLISH_STATE_ERROR",
                format!("failed to create {}: {err}", path.display()),
            )
        })?;
        let previous = env::current_dir().map_err(|err| {
            AppError::new(
                "PUBLISH_STATE_ERROR",
                format!("failed to read current directory: {err}"),
            )
        })?;
        env::set_current_dir(path).map_err(|err| {
            AppError::new(
                "PUBLISH_STATE_ERROR",
                format!("failed to enter {}: {err}", path.display()),
            )
        })?;

        Ok(Self { previous })
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};
    use std::collections::HashMap;
    use typub_core::{Content, ContentFormat, ContentMeta};
    use typub_storage::PublishResult;

    #[test]
    fn publish_context_uses_stage_root_typub_status_db() {
        let temp = tempfile::tempdir().expect("temp dir");
        let stage_root = temp.path().join("stage");

        with_typub_status_lock(|| {
            let before = env::current_dir().expect("current dir");

            let _ctx = new_publish_context_with_stage_status_locked(
                &minimal_typub_config(&stage_root),
                &stage_root,
            )?;

            assert_eq!(env::current_dir().expect("current dir"), before);
            Ok(())
        })
        .expect("publish context");
        assert!(typub_status_db_path(&stage_root).exists());
    }

    #[test]
    fn join_sync_items_reads_remote_fields_from_typub_status() {
        let temp = tempfile::tempdir().expect("temp dir");
        let stage_root = temp.path().join("stage");
        let slug = "projects-cuda-agent-notes";
        seed_typub_status(
            &stage_root,
            slug,
            "https://example.atlassian.net/wiki/spaces/GPU/pages/42/Notes",
            "42",
        );

        let mut items = vec![SyncItemResult {
            path: "projects/cuda-agent/notes.typ".to_string(),
            title: "Notes".to_string(),
            slug: slug.to_string(),
            parent_path: None,
            parent_id: None,
            action: "deleted".to_string(),
            status: "skipped".to_string(),
            fingerprint: None,
            previous_fingerprint: Some("old".to_string()),
            url: None,
            platform_id: None,
            archive_task_id: None,
            error: None,
            reason: None,
        }];

        join_sync_items_with_typub_status(&stage_root, &mut items).expect("join status");

        assert_eq!(items[0].platform_id.as_deref(), Some("42"));
        assert_eq!(
            items[0].url.as_deref(),
            Some("https://example.atlassian.net/wiki/spaces/GPU/pages/42/Notes")
        );
    }

    #[test]
    fn join_sync_items_reclassifies_known_remote_create_as_update() {
        let temp = tempfile::tempdir().expect("temp dir");
        let stage_root = temp.path().join("stage");
        let slug = "projects-cuda-agent-notes";
        seed_typub_status(
            &stage_root,
            slug,
            "https://example.atlassian.net/wiki/spaces/GPU/pages/42/Notes",
            "42",
        );

        let mut items = vec![SyncItemResult {
            path: "projects/cuda-agent/notes.typ".to_string(),
            title: "Notes".to_string(),
            slug: slug.to_string(),
            parent_path: None,
            parent_id: None,
            action: "create".to_string(),
            status: "pending".to_string(),
            fingerprint: Some("new".to_string()),
            previous_fingerprint: None,
            url: None,
            platform_id: None,
            archive_task_id: None,
            error: None,
            reason: Some("not present in local publish state".to_string()),
        }];

        join_sync_items_with_typub_status(&stage_root, &mut items).expect("join status");

        assert_eq!(items[0].action, "update");
        assert_eq!(items[0].platform_id.as_deref(), Some("42"));
        assert_eq!(
            items[0].reason.as_deref(),
            Some("present in typub status but missing from local publish state")
        );
    }

    fn seed_typub_status(stage_root: &Path, slug: &str, url: &str, platform_id: &str) {
        let config = minimal_typub_config(stage_root);
        let mut ctx =
            new_publish_context_with_stage_status(&config, stage_root).expect("publish context");
        let content = staged_content(stage_root, slug);
        let result = PublishResult {
            url: Some(url.to_string()),
            platform_id: Some(platform_id.to_string()),
            published_at: Utc::now(),
        };

        ctx.status
            .mark_published(&content, CONFLUENCE_PLATFORM, &result, Some("published"))
            .expect("mark published");
    }

    fn staged_content(stage_root: &Path, slug: &str) -> Content {
        let post_dir = stage_root.join("posts").join(slug);
        fs::create_dir_all(&post_dir).expect("create post dir");
        let content_file = post_dir.join("content.md");
        fs::write(&content_file, "# Notes\n").expect("write content");

        Content {
            path: post_dir,
            meta: ContentMeta {
                title: "Notes".to_string(),
                created: NaiveDate::from_ymd_opt(2026, 6, 18).expect("date"),
                updated: None,
                tags: Vec::new(),
                categories: Vec::new(),
                published: Some(true),
                theme: None,
                internal_link_target: None,
                preamble: None,
                platforms: HashMap::new(),
            },
            content_file,
            source_format: ContentFormat::Markdown,
            slides_file: None,
            assets: Vec::new(),
        }
    }

    fn minimal_typub_config(stage_root: &Path) -> TypubConfig {
        TypubConfig {
            content_dir: stage_root.join("posts"),
            output_dir: stage_root.join("output"),
            storage: None,
            published: Some(true),
            theme: None,
            internal_link_target: None,
            preamble: None,
            platforms: HashMap::new(),
        }
    }
}
