use super::publishing::load_publish_state_view;
use crate::domain::*;
use crate::infrastructure::*;
use crate::support::*;
use serde_json::json;
use std::path::PathBuf;

pub(crate) fn cmd_root(
    dir: Option<PathBuf>,
    base_url: Option<String>,
) -> AppResult<serde_json::Value> {
    let paths = config_paths()?;
    let dir = dir.or_else(|| env_path(ENV_KB_ROOT)).ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_ROOT",
            format!("pass a root directory or set {ENV_KB_ROOT}"),
        )
    })?;
    let root = expand_tilde(&dir)?;

    if !root.exists() {
        return Err(AppError::new(
            "ROOT_NOT_FOUND",
            format!("root directory does not exist: {}", root.display()),
        ));
    }

    if !root.is_dir() {
        return Err(AppError::new(
            "ROOT_NOT_DIRECTORY",
            format!("root is not a directory: {}", root.display()),
        ));
    }

    let mut config = load_user_config(&paths.user_config)?.unwrap_or_default();
    config.root = Some(root);
    let base_url = base_url.or_else(|| env_string(ENV_BASE_URL));
    if base_url.is_some() {
        config.base_url = base_url;
    }
    write_toml(&paths.user_config, &config)?;

    Ok(ok(json!({
        "user_config": display_path(&paths.user_config),
        "root": config.root.as_ref().map(|path| display_path(path)),
        "base_url": config.base_url,
    })))
}

pub(crate) fn cmd_bind(
    source: String,
    space: Option<String>,
    parent_id: Option<String>,
    base_url: Option<String>,
) -> AppResult<serde_json::Value> {
    let paths = config_paths()?;
    let user = required_user_config(&paths.user_config)?;
    let root = required_root(&user)?;
    let space = space.or_else(|| env_string(ENV_SPACE)).ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_SPACE",
            format!("pass --space <space> or set {ENV_SPACE}"),
        )
    })?;
    let parent_id = parent_id
        .or_else(|| env_string(ENV_PARENT_ID))
        .ok_or_else(|| {
            AppError::new(
                "CONFIG_MISSING_PARENT",
                format!("pass --parent <page_id> or set {ENV_PARENT_ID}"),
            )
        })?;
    let source_abs = root.join(&source);

    if !source_abs.exists() {
        return Err(AppError::new(
            "SOURCE_NOT_FOUND",
            format!("source does not exist under root: {}", source_abs.display()),
        ));
    }

    if !source_abs.is_dir() {
        return Err(AppError::new(
            "SOURCE_NOT_DIRECTORY",
            format!("source is not a directory: {}", source_abs.display()),
        ));
    }

    // Re-binding must not drop credentials an operator added to the project
    // config by hand; they are preserved but never echoed into the output.
    let existing_confluence =
        load_project_config(&paths.project_config)?.and_then(|existing| existing.confluence);

    let project = ProjectConfig {
        source,
        space,
        parent_id,
        base_url: base_url.or_else(|| env_string(ENV_BASE_URL)),
        confluence: existing_confluence,
    };
    write_toml(&paths.project_config, &project)?;

    Ok(ok(json!({
        "project_config": display_path(&paths.project_config),
        "binding": {
            "source": project.source,
            "space": project.space,
            "parent_id": project.parent_id,
            "base_url": project.base_url,
        },
    })))
}

pub(crate) fn cmd_status() -> AppResult<serde_json::Value> {
    let paths = config_paths()?;
    let user = effective_user_config(&paths.user_config)?;
    let project = load_project_config(&paths.project_config)?;
    let (resolved, docs, sync) = if user.root.is_some() && project.is_some() {
        let view = load_publish_state_view()?;
        let (items, publish_snapshots) =
            build_sync_plan(&view.snapshots, &view.state, true, &view.resolved.source);

        (
            Some(view.resolved),
            view.documents.len(),
            Some(json!({
                "state_file": display_path(&view.state_path),
                "tracked": view.state.documents.len(),
                "publishable": publish_snapshots.len(),
                "summary": sync_counts(&items),
            })),
        )
    } else {
        (None, 0, None)
    };

    Ok(ok(json!({
        "user_config": {
            "path": display_path(&paths.user_config),
            "exists": paths.user_config.exists(),
        },
        "project_config": {
            "path": display_path(&paths.project_config),
            "exists": paths.project_config.exists(),
        },
        "resolved": resolved,
        "documents": docs,
        "sync": sync,
    })))
}
