use serde_json::json;
use std::env;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const APP_DIR: &str = "conpub";
pub(crate) const USER_CONFIG_FILE: &str = "conpub.toml";
pub(crate) const PROJECT_CONFIG_FILE: &str = ".conpub.toml";
pub(crate) const SYNC_STATE_FILE: &str = "sync-state.json";
pub(crate) const SEARCH_INDEX_FILE: &str = "search-index.json";
pub(crate) const SUPPORTED_EXTENSIONS: [&str; 2] = ["md", "typ"];
pub(crate) const ENV_KB_ROOT: &str = "CONPUB_KB_ROOT";
pub(crate) const ENV_BASE_URL: &str = "CONPUB_BASE_URL";
pub(crate) const ENV_SPACE: &str = "CONPUB_SPACE";
pub(crate) const ENV_PARENT_ID: &str = "CONPUB_PARENT_ID";
// Shared with the typub confluence adapter, which also reads these itself;
// conpub resolves them here so config-file credentials can participate.
pub(crate) const ENV_CONFLUENCE_API_KEY: &str = "CONFLUENCE_API_KEY";
pub(crate) const ENV_CONFLUENCE_EMAIL: &str = "CONFLUENCE_EMAIL";

#[derive(Debug)]
pub(crate) struct AppError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    pub(crate) exit_code: i32,
}

pub(crate) type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            exit_code: 1,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

pub(crate) fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn normalize_confluence_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed.strip_suffix("/wiki").unwrap_or(trimmed).to_string()
}

pub(crate) fn slug_for_path(path: &str) -> String {
    let without_ext = path.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(path);
    sanitize_slug(without_ext)
}

pub(crate) fn sanitize_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }

    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "document".to_string()
    } else {
        slug
    }
}

pub(crate) fn validate_publish_pacing(concurrency: usize) -> AppResult<()> {
    if concurrency == 1 {
        return Ok(());
    }

    Err(AppError::new(
        "CONCURRENCY_UNSUPPORTED",
        "conpub currently supports only --concurrency 1 because typub publish state is serialized",
    ))
}

pub(crate) fn build_async_runtime() -> AppResult<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| AppError::new("RUNTIME_ERROR", format!("failed to start runtime: {err}")))
}

pub(crate) fn publish_pacing_json(delay_ms: u64, concurrency: usize) -> serde_json::Value {
    json!({
        "delay_ms": delay_ms,
        "concurrency": concurrency,
    })
}

pub(crate) fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_path(name: &str) -> Option<PathBuf> {
    env_string(name).map(PathBuf::from)
}

pub(crate) fn canonical_or_original(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

pub(crate) fn display_path(path: &Path) -> String {
    path.display().to_string()
}

pub(crate) fn path_to_slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn ok(data: serde_json::Value) -> serde_json::Value {
    json!({
        "ok": true,
        "data": data,
    })
}

pub(crate) fn write_json(value: &serde_json::Value, pretty: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if pretty {
        serde_json::to_writer_pretty(&mut out, value)?;
    } else {
        serde_json::to_writer(&mut out, value)?;
    }
    writeln!(out)
}

pub(crate) fn write_error(err: &AppError, pretty: bool) -> io::Result<()> {
    let value = json!({
        "ok": false,
        "code": err.code,
        "message": err.message,
        "retryable": err.retryable,
    });
    write_json(&value, pretty)
}
