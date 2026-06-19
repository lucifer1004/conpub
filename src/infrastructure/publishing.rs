use crate::domain::*;
use crate::infrastructure::assets::stage_shared_assets;
use crate::infrastructure::config::conpub_home;
use crate::infrastructure::{get_platform_id, new_publish_context_with_stage_status};
use crate::support::*;
use chrono::NaiveDate;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use typub_config::{Config as TypubConfig, PlatformConfig};
use typub_core::{Content, ContentFormat, ContentMeta, PostPlatformConfig};
use typub_engine::{AdapterRegistry, Renderer, publish_single_platform};
use typub_storage::StatusTracker;

pub(crate) async fn publish_snapshots_with_hierarchy(
    config: &TypubConfig,
    resolved: &ResolvedConfig,
    stage_root: &Path,
    snapshots: &[DocumentSnapshot],
    state: &SyncState,
    delay_ms: u64,
) -> AppResult<Vec<PublishItemResult>> {
    let registry = AdapterRegistry::new(config)
        .map_err(|err| AppError::new("PUBLISH_BACKEND_ERROR", err.to_string()))?;
    let adapter = registry
        .get("confluence")
        .map_err(|err| AppError::new("PUBLISH_BACKEND_ERROR", err.to_string()))?;

    if let Some(platform_config) = config.get_platform("confluence") {
        adapter
            .validate_config(platform_config)
            .map_err(|err| AppError::new("PUBLISH_CONFIG_ERROR", err.to_string()))?;
    }

    let renderer = Renderer::new_with_root(config, stage_root.to_path_buf());
    let mut ctx = new_publish_context_with_stage_status(config, stage_root)?;
    let mut published_ids = HashMap::new();
    let mut results = Vec::new();

    let mut ordered = snapshots.to_vec();
    ordered.sort_by(|a, b| {
        a.hierarchy_order
            .cmp(&b.hierarchy_order)
            .then(a.document.path.cmp(&b.document.path))
    });

    for (idx, snapshot) in ordered.into_iter().enumerate() {
        if idx > 0 && delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        let parent_id = match resolve_snapshot_parent_id(
            &snapshot,
            state,
            &published_ids,
            resolved,
            &ctx.status,
        ) {
            Ok(parent_id) => parent_id,
            Err(err) => {
                results.push(PublishItemResult {
                    path: snapshot.document.path,
                    title: snapshot.document.title,
                    slug: snapshot.slug,
                    parent_path: snapshot.parent_path,
                    parent_id: None,
                    status: "failed".to_string(),
                    url: None,
                    platform_id: None,
                    error: Some(err.to_string()),
                });
                continue;
            }
        };
        let content = stage_document(resolved, &snapshot.document, stage_root, &parent_id)?;
        let result = publish_single_platform(
            adapter,
            "confluence",
            &content,
            &renderer,
            &mut ctx,
            config,
            None,
        )
        .await;

        match result {
            Ok(published) => {
                if let Some(platform_id) = &published.platform_id {
                    published_ids.insert(snapshot.document.path.clone(), platform_id.clone());
                }
                results.push(PublishItemResult {
                    path: snapshot.document.path,
                    title: snapshot.document.title,
                    slug: snapshot.slug,
                    parent_path: snapshot.parent_path,
                    parent_id: Some(parent_id),
                    status: "published".to_string(),
                    url: published.url,
                    platform_id: published.platform_id,
                    error: None,
                });
            }
            Err(err) => results.push(PublishItemResult {
                path: snapshot.document.path,
                title: snapshot.document.title,
                slug: snapshot.slug,
                parent_path: snapshot.parent_path,
                parent_id: Some(parent_id),
                status: "failed".to_string(),
                url: None,
                platform_id: None,
                error: Some(err.to_string()),
            }),
        }
    }

    ctx.status
        .save()
        .map_err(|err| AppError::new("PUBLISH_STATE_ERROR", err.to_string()))?;

    Ok(results)
}

pub(crate) fn resolve_snapshot_parent_id(
    snapshot: &DocumentSnapshot,
    state: &SyncState,
    published_ids: &HashMap<String, String>,
    resolved: &ResolvedConfig,
    typub_status: &StatusTracker,
) -> AppResult<String> {
    let Some(parent_path) = &snapshot.parent_path else {
        return Ok(resolved.target.parent_id.clone());
    };

    if let Some(parent_id) = published_ids.get(parent_path) {
        return Ok(parent_id.clone());
    }

    if let Some(entry) = state.documents.get(parent_path)
        && let Some(parent_id) = get_platform_id(typub_status, &entry.slug)?
    {
        return Ok(parent_id);
    }

    Err(AppError::new(
        "HIERARCHY_PARENT_NOT_PUBLISHED",
        format!(
            "parent page for {} is not published yet: {parent_path}",
            snapshot.document.path
        ),
    ))
}

pub(crate) fn build_typub_config(
    resolved: &ResolvedConfig,
    stage_root: &Path,
) -> AppResult<TypubConfig> {
    let base_url = resolved.target.base_url.as_deref().ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_BASE_URL",
            format!(
                "run `conpub root <dir> --base-url <url>`, bind with --base-url, or set {ENV_BASE_URL}"
            ),
        )
    })?;

    let mut extra = HashMap::new();
    extra.insert(
        "base_url".to_string(),
        toml::Value::String(normalize_confluence_base_url(base_url)),
    );
    extra.insert(
        "space".to_string(),
        toml::Value::String(resolved.target.space.clone()),
    );
    extra.insert(
        "parent_id".to_string(),
        toml::Value::String(resolved.target.parent_id.clone()),
    );

    let mut platforms = HashMap::new();
    platforms.insert(
        "confluence".to_string(),
        PlatformConfig {
            enabled: true,
            asset_strategy: Some("upload".to_string()),
            published: Some(true),
            theme: None,
            internal_link_target: None,
            math_rendering: Some("latex".to_string()),
            math_delimiters: None,
            extra,
        },
    );

    Ok(TypubConfig {
        content_dir: stage_root.join("posts"),
        output_dir: stage_root.join("output"),
        storage: None,
        published: Some(true),
        theme: None,
        internal_link_target: None,
        preamble: None,
        platforms,
    })
}

pub(crate) fn stage_document(
    resolved: &ResolvedConfig,
    document: &Document,
    stage_root: &Path,
    parent_id: &str,
) -> AppResult<Content> {
    let staged = stage_document_files(resolved, document, stage_root)?;

    Ok(Content {
        path: staged.post_dir,
        meta: content_meta_for(document, &resolved.target, parent_id)?,
        content_file: staged.content_file,
        source_format: staged.source_format,
        slides_file: None,
        assets: staged.assets,
    })
}

pub(crate) fn stage_document_files(
    resolved: &ResolvedConfig,
    document: &Document,
    stage_root: &Path,
) -> AppResult<StagedDocumentFiles> {
    let root = PathBuf::from(&resolved.root);
    let posts_dir = stage_root.join("posts");
    fs::create_dir_all(&posts_dir).map_err(|err| {
        AppError::new(
            "STAGE_WRITE_ERROR",
            format!("failed to create {}: {err}", posts_dir.display()),
        )
    })?;

    let original = root.join(&document.path);
    let slug = slug_for_path(&document.path);
    let post_dir = posts_dir.join(&slug);

    if post_dir.exists() {
        fs::remove_dir_all(&post_dir).map_err(|err| {
            AppError::new(
                "STAGE_WRITE_ERROR",
                format!("failed to reset {}: {err}", post_dir.display()),
            )
        })?;
    }
    fs::create_dir_all(&post_dir).map_err(|err| {
        AppError::new(
            "STAGE_WRITE_ERROR",
            format!("failed to create {}: {err}", post_dir.display()),
        )
    })?;

    let source_format = match document.extension.as_str() {
        "typ" => ContentFormat::Typst,
        _ => ContentFormat::Markdown,
    };
    let staged_name = match source_format {
        ContentFormat::Typst => "content.typ",
        ContentFormat::Markdown => "content.md",
    };
    let assets = stage_shared_assets(&root, &post_dir)?;
    let content_file = post_dir.join(staged_name);
    fs::copy(&original, &content_file).map_err(|err| {
        AppError::new(
            "STAGE_WRITE_ERROR",
            format!(
                "failed to stage {} as {}: {err}",
                original.display(),
                content_file.display()
            ),
        )
    })?;

    Ok(StagedDocumentFiles {
        post_dir,
        content_file,
        source_format,
        assets,
    })
}

pub(crate) struct StagedDocumentFiles {
    pub(crate) post_dir: PathBuf,
    pub(crate) content_file: PathBuf,
    pub(crate) source_format: ContentFormat,
    pub(crate) assets: Vec<PathBuf>,
}

pub(crate) fn content_meta_for(
    document: &Document,
    target: &Target,
    parent_id: &str,
) -> AppResult<ContentMeta> {
    let created = NaiveDate::from_ymd_opt(1970, 1, 1)
        .ok_or_else(|| AppError::new("DATE_ERROR", "failed to construct default date"))?;
    let mut extra = HashMap::new();
    extra.insert(
        "space".to_string(),
        toml::Value::String(target.space.clone()),
    );
    extra.insert(
        "parent_id".to_string(),
        toml::Value::String(parent_id.to_string()),
    );
    let mut platforms = HashMap::new();
    platforms.insert(
        "confluence".to_string(),
        PostPlatformConfig {
            published: Some(true),
            internal_link_target: None,
            extra,
        },
    );

    Ok(ContentMeta {
        title: document.title.clone(),
        created,
        updated: None,
        tags: Vec::new(),
        categories: Vec::new(),
        published: Some(true),
        theme: None,
        internal_link_target: None,
        preamble: None,
        platforms,
    })
}

pub(crate) fn publish_stage_root(resolved: &ResolvedConfig) -> AppResult<PathBuf> {
    let key = format!("{}-{}", resolved.target.space, resolved.target.parent_id);
    Ok(conpub_home()?.join("typub-stage").join(sanitize_slug(&key)))
}
