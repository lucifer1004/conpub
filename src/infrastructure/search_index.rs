use crate::domain::*;
use crate::infrastructure::assets::snapshot_documents;
use crate::infrastructure::filesystem::{list_documents, read_text_file};
use crate::infrastructure::publishing::publish_stage_root;
use crate::support::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn search_index_path(resolved: &ResolvedConfig, all: bool) -> AppResult<PathBuf> {
    let name = if all {
        "search-index-root.json"
    } else {
        SEARCH_INDEX_FILE
    };
    Ok(publish_stage_root(resolved)?.join(name))
}

pub(crate) fn search_index_identity(resolved: &ResolvedConfig, all: bool) -> SearchIndexIdentity {
    SearchIndexIdentity {
        root: resolved.root.clone(),
        source: resolved.source.clone(),
        scope: if all { "root" } else { "source" }.to_string(),
    }
}

pub(crate) fn build_search_index(
    resolved: &ResolvedConfig,
    root: &Path,
    scope: &Path,
    all: bool,
) -> AppResult<SearchIndex> {
    let documents = list_documents(root, scope)?;
    let snapshots = snapshot_documents(root, &documents)?;
    let fingerprints = snapshots
        .iter()
        .map(|snapshot| (snapshot.document.path.clone(), snapshot.fingerprint.clone()))
        .collect::<HashMap<_, _>>();
    let mut index_documents = HashMap::new();

    for document in documents {
        let content = read_text_file(&root.join(&document.path))?;
        index_documents.insert(
            document.path.clone(),
            SearchIndexDocument {
                fingerprint: fingerprints
                    .get(&document.path)
                    .cloned()
                    .unwrap_or_default(),
                title: document.title,
                lines: content.lines().map(str::to_string).collect(),
            },
        );
    }

    Ok(SearchIndex {
        version: 1,
        identity: search_index_identity(resolved, all),
        documents: index_documents,
    })
}

pub(crate) fn load_fresh_search_index(
    resolved: &ResolvedConfig,
    root: &Path,
    scope: &Path,
    all: bool,
    path: &Path,
) -> AppResult<Option<SearchIndex>> {
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(path).map_err(|err| {
        AppError::new(
            "INDEX_READ_ERROR",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let index: SearchIndex = serde_json::from_str(&text).map_err(|err| {
        AppError::new(
            "INDEX_PARSE_ERROR",
            format!("failed to parse {}: {err}", path.display()),
        )
    })?;

    if index.version != 1 || index.identity != search_index_identity(resolved, all) {
        return Ok(None);
    }

    let documents = list_documents(root, scope)?;
    if documents.len() != index.documents.len() {
        return Ok(None);
    }

    let snapshots = snapshot_documents(root, &documents)?;
    for snapshot in snapshots {
        let Some(indexed) = index.documents.get(&snapshot.document.path) else {
            return Ok(None);
        };

        if indexed.fingerprint != snapshot.fingerprint {
            return Ok(None);
        }
    }

    Ok(Some(index))
}

pub(crate) fn write_search_index(path: &Path, index: &SearchIndex) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::new(
                "INDEX_WRITE_ERROR",
                format!("failed to create {}: {err}", parent.display()),
            )
        })?;
    }

    let text = serde_json::to_string_pretty(index)
        .map_err(|err| AppError::new("INDEX_ENCODE_ERROR", err.to_string()))?;
    fs::write(path, text).map_err(|err| {
        AppError::new(
            "INDEX_WRITE_ERROR",
            format!("failed to write {}: {err}", path.display()),
        )
    })
}
