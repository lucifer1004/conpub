use super::models::{Document, HierarchyEntry, ResolvedConfig};
use crate::support::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(crate) fn validate_directory_index_conflicts(
    resolved: &ResolvedConfig,
    documents: &[Document],
) -> AppResult<()> {
    build_directory_index_map(resolved, documents).map(|_| ())
}

pub(crate) fn build_hierarchy(
    resolved: &ResolvedConfig,
    selected_documents: &[Document],
    all_documents: &[Document],
) -> AppResult<Vec<HierarchyEntry>> {
    let index_by_dir = build_directory_index_map(resolved, all_documents)?;
    let all_by_path = all_documents
        .iter()
        .map(|document| (document.path.as_str(), document))
        .collect::<HashMap<_, _>>();
    let mut entries = HashMap::new();

    for document in selected_documents {
        add_hierarchy_entry(
            resolved,
            &document.path,
            &all_by_path,
            &index_by_dir,
            &mut entries,
        )?;
    }

    let mut entries = entries.into_values().collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then(a.document.path.cmp(&b.document.path))
    });
    Ok(entries)
}

pub(crate) fn add_hierarchy_entry(
    resolved: &ResolvedConfig,
    path: &str,
    all_by_path: &HashMap<&str, &Document>,
    index_by_dir: &HashMap<String, String>,
    entries: &mut HashMap<String, HierarchyEntry>,
) -> AppResult<()> {
    if entries.contains_key(path) {
        return Ok(());
    }

    let document = all_by_path.get(path).ok_or_else(|| {
        AppError::new(
            "DOCUMENT_NOT_FOUND",
            format!("document is not part of the bound source: {path}"),
        )
    })?;
    let (parent_path, order) = hierarchy_parent_and_order(resolved, document, index_by_dir)?;

    if let Some(parent_path) = &parent_path {
        add_hierarchy_entry(resolved, parent_path, all_by_path, index_by_dir, entries)?;
    }

    entries.insert(
        path.to_string(),
        HierarchyEntry {
            document: (*document).clone(),
            parent_path,
            order,
        },
    );
    Ok(())
}

pub(crate) fn build_directory_index_map(
    resolved: &ResolvedConfig,
    documents: &[Document],
) -> AppResult<HashMap<String, String>> {
    let mut index_by_dir = HashMap::new();

    for document in documents {
        if !is_index_document(&document.path) {
            continue;
        }

        let source_rel = source_relative_path(resolved, document)?;
        let dir = source_relative_parent_dir(&source_rel);
        if let Some(previous) = index_by_dir.insert(dir.clone(), document.path.clone()) {
            return Err(AppError::new(
                "HIERARCHY_INDEX_CONFLICT",
                format!(
                    "multiple directory index documents for source-relative directory `{}`: {} and {}",
                    display_source_dir(&dir),
                    previous,
                    document.path
                ),
            ));
        }
    }

    Ok(index_by_dir)
}

pub(crate) fn hierarchy_parent_and_order(
    resolved: &ResolvedConfig,
    document: &Document,
    index_by_dir: &HashMap<String, String>,
) -> AppResult<(Option<String>, usize)> {
    let source_rel = source_relative_path(resolved, document)?;
    let dir = source_relative_parent_dir(&source_rel);
    let depth = directory_depth(&dir);

    if is_index_document(&document.path) {
        if dir.is_empty() {
            return Ok((None, 0));
        }

        let parent_dir = parent_dir_string(&dir);
        let parent_path = if parent_dir.is_empty() {
            None
        } else {
            Some(required_directory_index(
                &parent_dir,
                &document.path,
                index_by_dir,
            )?)
        };
        return Ok((parent_path, depth));
    }

    if dir.is_empty() {
        return Ok((None, 1));
    }

    Ok((
        Some(required_directory_index(
            &dir,
            &document.path,
            index_by_dir,
        )?),
        depth + 1,
    ))
}

pub(crate) fn required_directory_index(
    dir: &str,
    child_path: &str,
    index_by_dir: &HashMap<String, String>,
) -> AppResult<String> {
    index_by_dir.get(dir).cloned().ok_or_else(|| {
        AppError::new(
            "HIERARCHY_INDEX_MISSING",
            format!(
                "missing _index.md or index.md for source-relative directory `{}` required by {}",
                display_source_dir(dir),
                child_path
            ),
        )
    })
}

pub(crate) fn source_relative_path(
    resolved: &ResolvedConfig,
    document: &Document,
) -> AppResult<PathBuf> {
    let document_path = PathBuf::from(&document.path);
    let source = PathBuf::from(&resolved.source);

    if resolved.source.is_empty() || resolved.source == "." {
        return Ok(document_path);
    }

    document_path
        .strip_prefix(&source)
        .map(Path::to_path_buf)
        .map_err(|_| {
            AppError::new(
                "PATH_OUTSIDE_SOURCE",
                format!(
                    "document is outside bound source {}: {}",
                    resolved.source, document.path
                ),
            )
        })
}

pub(crate) fn source_relative_parent_dir(path: &Path) -> String {
    path.parent()
        .map(path_to_slash)
        .filter(|dir| !dir.is_empty())
        .unwrap_or_default()
}

pub(crate) fn parent_dir_string(dir: &str) -> String {
    Path::new(dir)
        .parent()
        .map(path_to_slash)
        .filter(|parent| !parent.is_empty())
        .unwrap_or_default()
}

pub(crate) fn directory_depth(dir: &str) -> usize {
    dir.split('/')
        .filter(|component| !component.is_empty())
        .count()
}

pub(crate) fn display_source_dir(dir: &str) -> &str {
    if dir.is_empty() { "." } else { dir }
}

pub(crate) fn is_index_document(path: &str) -> bool {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem == "_index" || stem == "index")
        .unwrap_or(false)
}

pub(crate) fn validate_unique_slugs(documents: &[Document]) -> AppResult<()> {
    let mut slugs = HashMap::new();
    for document in documents {
        let slug = slug_for_path(&document.path);
        if let Some(previous) = slugs.insert(slug.clone(), document.path.clone()) {
            return Err(AppError::new(
                "SLUG_COLLISION",
                format!(
                    "documents produce the same slug `{slug}`: {previous} and {}",
                    document.path
                ),
            ));
        }
    }

    Ok(())
}
