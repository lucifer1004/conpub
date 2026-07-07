use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) const SYNC_STATE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct UserConfig {
    pub(crate) root: Option<PathBuf>,
    pub(crate) base_url: Option<String>,
    pub(crate) confluence: Option<ConfluenceCredentials>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectConfig {
    pub(crate) source: String,
    pub(crate) space: String,
    pub(crate) parent_id: String,
    pub(crate) base_url: Option<String>,
    pub(crate) confluence: Option<ConfluenceCredentials>,
}

/// Confluence credentials from `[confluence]` in the user or project config.
/// Resolved env-over-project-over-user and handed to the typub platform
/// config in memory only — never serialized into resolve/plan output.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ConfluenceCredentials {
    pub(crate) api_key: Option<String>,
    pub(crate) email: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ConfigPaths {
    pub(crate) user_config: PathBuf,
    pub(crate) project_config: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Target {
    pub(crate) base_url: Option<String>,
    pub(crate) space: String,
    pub(crate) parent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedConfig {
    pub(crate) root: String,
    pub(crate) source: String,
    pub(crate) source_abs: String,
    pub(crate) target: Target,
    pub(crate) user_config: String,
    pub(crate) project_config: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Document {
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) extension: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchMatch {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) read_ref: String,
    pub(crate) title: String,
    pub(crate) snippet: String,
    pub(crate) confluence_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SearchIndex {
    pub(crate) version: u32,
    pub(crate) identity: SearchIndexIdentity,
    pub(crate) documents: HashMap<String, SearchIndexDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SearchIndexIdentity {
    pub(crate) root: String,
    pub(crate) source: String,
    pub(crate) scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SearchIndexDocument {
    pub(crate) fingerprint: String,
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReadLine {
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ReadResult {
    pub(crate) path: String,
    pub(crate) target_line: usize,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) title: String,
    pub(crate) lines: Vec<ReadLine>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PlanItem {
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) action: String,
    pub(crate) reason: String,
    pub(crate) confluence_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PublishItemResult {
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) parent_path: Option<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) status: String,
    pub(crate) url: Option<String>,
    pub(crate) platform_id: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SyncItemResult {
    pub(crate) path: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    pub(crate) parent_path: Option<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) action: String,
    pub(crate) status: String,
    pub(crate) fingerprint: Option<String>,
    pub(crate) previous_fingerprint: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) platform_id: Option<String>,
    pub(crate) archive_task_id: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentSnapshot {
    pub(crate) document: Document,
    pub(crate) slug: String,
    pub(crate) fingerprint: String,
    pub(crate) parent_path: Option<String>,
    pub(crate) hierarchy_order: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct HierarchyEntry {
    pub(crate) document: Document,
    pub(crate) parent_path: Option<String>,
    pub(crate) order: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncState {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) identity: Option<SyncStateIdentity>,
    pub(crate) documents: HashMap<String, SyncStateDocument>,
}

impl SyncState {
    pub(crate) fn new(identity: SyncStateIdentity) -> Self {
        Self {
            version: SYNC_STATE_VERSION,
            identity: Some(identity),
            documents: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncStateIdentity {
    pub(crate) root: String,
    pub(crate) source: String,
    pub(crate) base_url: Option<String>,
    pub(crate) space: String,
    pub(crate) parent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncStateDocument {
    pub(crate) fingerprint: String,
    pub(crate) title: String,
    pub(crate) slug: String,
    #[serde(default)]
    pub(crate) parent_path: Option<String>,
    pub(crate) synced_at: u64,
}
