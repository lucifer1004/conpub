use crate::domain::*;
use crate::support::*;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn resolve_config() -> AppResult<ResolvedConfig> {
    let paths = config_paths()?;
    let user = required_user_config(&paths.user_config)?;
    let project = required_project_config(&paths.project_config)?;
    let root = required_root(&user)?;
    let source_abs = root.join(&project.source);

    if !source_abs.exists() {
        return Err(AppError::new(
            "SOURCE_NOT_FOUND",
            format!("source does not exist under root: {}", source_abs.display()),
        ));
    }

    let root = canonical_or_original(root);
    let source_abs = canonical_or_original(source_abs);
    let base_url = project.base_url.clone().or(user.base_url.clone());

    Ok(ResolvedConfig {
        root: display_path(&root),
        source: project.source.clone(),
        source_abs: display_path(&source_abs),
        target: Target {
            base_url,
            space: project.space.clone(),
            parent_id: project.parent_id.clone(),
        },
        user_config: display_path(&paths.user_config),
        project_config: display_path(&paths.project_config),
    })
}

pub(crate) fn config_paths() -> AppResult<ConfigPaths> {
    let user_config = conpub_home()?.join(USER_CONFIG_FILE);
    let project_config = find_project_config()?
        .unwrap_or(env::current_dir().map_err(|err| {
            AppError::new(
                "CURRENT_DIR_ERROR",
                format!("failed to read current directory: {err}"),
            )
        })?)
        .join(PROJECT_CONFIG_FILE);

    Ok(ConfigPaths {
        user_config,
        project_config,
    })
}

pub(crate) fn conpub_home() -> AppResult<PathBuf> {
    if let Some(value) = env::var_os("CONPUB_HOME") {
        return Ok(PathBuf::from(value));
    }

    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home).join(APP_DIR));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        AppError::new(
            "HOME_NOT_SET",
            "HOME is not set; set CONPUB_HOME to choose a config directory",
        )
    })?;
    Ok(PathBuf::from(home).join(".config").join(APP_DIR))
}

pub(crate) fn find_project_config() -> AppResult<Option<PathBuf>> {
    let mut current = env::current_dir().map_err(|err| {
        AppError::new(
            "CURRENT_DIR_ERROR",
            format!("failed to read current directory: {err}"),
        )
    })?;

    loop {
        if current.join(PROJECT_CONFIG_FILE).exists() {
            return Ok(Some(current));
        }

        if !current.pop() {
            return Ok(None);
        }
    }
}

pub(crate) fn required_user_config(path: &Path) -> AppResult<UserConfig> {
    let config = effective_user_config(path)?;
    if config.root.is_some() {
        return Ok(config);
    }

    Err(AppError::new(
        "CONFIG_MISSING_ROOT",
        format!("run `conpub root <dir>` or set {ENV_KB_ROOT} before using project commands"),
    ))
}

pub(crate) fn effective_user_config(path: &Path) -> AppResult<UserConfig> {
    let mut config = load_user_config(path)?.unwrap_or_default();
    if config.root.is_none() {
        config.root = env_path(ENV_KB_ROOT);
    }
    if config.base_url.is_none() {
        config.base_url = env_string(ENV_BASE_URL);
    }
    Ok(config)
}

pub(crate) fn required_project_config(path: &Path) -> AppResult<ProjectConfig> {
    load_project_config(path)?.ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_BINDING",
            format!(
                "run `conpub bind <source>` in this project; target defaults can come from --space/--parent/--base-url or {ENV_SPACE}/{ENV_PARENT_ID}/{ENV_BASE_URL}"
            ),
        )
    })
}

pub(crate) fn required_root(config: &UserConfig) -> AppResult<PathBuf> {
    config.root.clone().ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_ROOT",
            format!("run `conpub root <dir>` or set {ENV_KB_ROOT} before using project commands"),
        )
    })
}

pub(crate) fn load_user_config(path: &Path) -> AppResult<Option<UserConfig>> {
    load_toml(path)
}

/// Resolve Confluence credentials for the typub adapter: environment first,
/// then the project `[confluence]` table, then the user one. The result is
/// injected into the in-memory typub platform config only; it never enters
/// resolve/plan/status output.
pub(crate) fn resolve_confluence_credentials() -> AppResult<ConfluenceCredentials> {
    let paths = config_paths()?;
    let user = load_user_config(&paths.user_config)?.unwrap_or_default();
    let project = load_project_config(&paths.project_config)?;
    Ok(merge_confluence_credentials(
        ConfluenceCredentials {
            api_key: env_string(ENV_CONFLUENCE_API_KEY),
            email: env_string(ENV_CONFLUENCE_EMAIL),
        },
        project.and_then(|config| config.confluence),
        user.confluence,
    ))
}

fn merge_confluence_credentials(
    env: ConfluenceCredentials,
    project: Option<ConfluenceCredentials>,
    user: Option<ConfluenceCredentials>,
) -> ConfluenceCredentials {
    let project = project.unwrap_or_default();
    let user = user.unwrap_or_default();
    ConfluenceCredentials {
        api_key: env.api_key.or(project.api_key).or(user.api_key),
        email: env.email.or(project.email).or(user.email),
    }
}

pub(crate) fn load_project_config(path: &Path) -> AppResult<Option<ProjectConfig>> {
    load_toml(path)
}

pub(crate) fn load_toml<T>(path: &Path) -> AppResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(path).map_err(|err| {
        AppError::new(
            "CONFIG_READ_ERROR",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let value = toml::from_str(&text).map_err(|err| {
        AppError::new(
            "CONFIG_PARSE_ERROR",
            format!("failed to parse {}: {err}", path.display()),
        )
    })?;

    Ok(Some(value))
}

pub(crate) fn write_toml<T>(path: &Path, value: &T) -> AppResult<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::new(
                "CONFIG_WRITE_ERROR",
                format!("failed to create {}: {err}", parent.display()),
            )
        })?;
    }

    let text = toml::to_string_pretty(value)
        .map_err(|err| AppError::new("CONFIG_ENCODE_ERROR", err.to_string()))?;
    fs::write(path, text).map_err(|err| {
        AppError::new(
            "CONFIG_WRITE_ERROR",
            format!("failed to write {}: {err}", path.display()),
        )
    })
}

#[cfg(test)]
mod credential_tests {
    #![allow(clippy::expect_used)]
    use super::*;

    fn creds(api_key: Option<&str>, email: Option<&str>) -> ConfluenceCredentials {
        ConfluenceCredentials {
            api_key: api_key.map(str::to_string),
            email: email.map(str::to_string),
        }
    }

    #[test]
    fn env_wins_over_project_over_user_per_field() {
        let merged = merge_confluence_credentials(
            creds(Some("env-key"), None),
            Some(creds(Some("project-key"), Some("project@x"))),
            Some(creds(Some("user-key"), Some("user@x"))),
        );
        assert_eq!(merged.api_key.as_deref(), Some("env-key"));
        assert_eq!(merged.email.as_deref(), Some("project@x"));
    }

    #[test]
    fn user_config_fills_when_nothing_else_set() {
        let merged = merge_confluence_credentials(
            creds(None, None),
            None,
            Some(creds(Some("k"), Some("e"))),
        );
        assert_eq!(merged.api_key.as_deref(), Some("k"));
        assert_eq!(merged.email.as_deref(), Some("e"));
    }
}

pub(crate) fn expand_tilde(path: &Path) -> AppResult<PathBuf> {
    let text = path.to_string_lossy();
    if text == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::new("HOME_NOT_SET", "HOME is not set"));
    }

    if let Some(rest) = text.strip_prefix("~/") {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::new("HOME_NOT_SET", "HOME is not set"))?;
        return Ok(home.join(rest));
    }

    Ok(path.to_path_buf())
}
