use crate::domain::*;
use crate::infrastructure::*;
use crate::support::*;
use serde_json::json;
use std::path::{Path, PathBuf};

pub(crate) fn cmd_search(query: &str, all: bool, limit: usize) -> AppResult<serde_json::Value> {
    let resolved = resolve_config()?;
    let root = PathBuf::from(&resolved.root);
    let scope = if all {
        root.clone()
    } else {
        PathBuf::from(&resolved.source_abs)
    };
    let needle = query.to_lowercase();
    let index_path = search_index_path(&resolved, all)?;
    if let Some(index) = load_fresh_search_index(&resolved, &root, &scope, all, &index_path)? {
        let matches = search_index_matches(&index, query, limit);
        return Ok(ok(json!({
            "query": query,
            "scope": if all { "root" } else { "source" },
            "limit": limit,
            "index": {
                "used": true,
                "path": display_path(&index_path),
            },
            "matches": matches,
        })));
    }

    let matches = scan_search_matches(&root, &scope, &needle, limit)?;

    Ok(ok(json!({
        "query": query,
        "scope": if all { "root" } else { "source" },
        "limit": limit,
        "index": {
            "used": false,
            "path": display_path(&index_path),
        },
        "matches": matches,
    })))
}

pub(crate) fn cmd_index(all: bool) -> AppResult<serde_json::Value> {
    let resolved = resolve_config()?;
    let root = PathBuf::from(&resolved.root);
    let scope = if all {
        root.clone()
    } else {
        PathBuf::from(&resolved.source_abs)
    };
    let index_path = search_index_path(&resolved, all)?;
    let index = build_search_index(&resolved, &root, &scope, all)?;
    write_search_index(&index_path, &index)?;

    Ok(ok(json!({
        "scope": if all { "root" } else { "source" },
        "index_file": display_path(&index_path),
        "documents": index.documents.len(),
    })))
}

pub(crate) fn scan_search_matches(
    root: &Path,
    scope: &Path,
    needle: &str,
    limit: usize,
) -> AppResult<Vec<SearchMatch>> {
    let mut matches = Vec::new();

    for doc in list_documents(root, scope)? {
        if matches.len() >= limit {
            break;
        }

        let path = root.join(&doc.path);
        let content = read_text_file(&path)?;
        for (idx, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(needle) {
                let line_number = idx + 1;
                matches.push(SearchMatch {
                    path: doc.path.clone(),
                    line: line_number,
                    read_ref: format!("{}:{line_number}", doc.path),
                    title: doc.title.clone(),
                    snippet: line.trim().to_string(),
                    confluence_url: None,
                });

                if matches.len() >= limit {
                    break;
                }
            }
        }
    }

    Ok(matches)
}

pub(crate) fn search_index_matches(
    index: &SearchIndex,
    query: &str,
    limit: usize,
) -> Vec<SearchMatch> {
    let needle = query.to_lowercase();
    let mut matches = Vec::new();
    let mut paths = index.documents.keys().collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        if matches.len() >= limit {
            break;
        }

        let Some(doc) = index.documents.get(path) else {
            continue;
        };

        for (idx, line) in doc.lines.iter().enumerate() {
            if line.to_lowercase().contains(&needle) {
                let line_number = idx + 1;
                matches.push(SearchMatch {
                    path: path.clone(),
                    line: line_number,
                    read_ref: format!("{path}:{line_number}"),
                    title: doc.title.clone(),
                    snippet: line.trim().to_string(),
                    confluence_url: None,
                });

                if matches.len() >= limit {
                    break;
                }
            }
        }
    }

    matches
}

pub(crate) fn cmd_read(
    reference: &str,
    explicit_line: Option<usize>,
    context: usize,
) -> AppResult<serde_json::Value> {
    let resolved = resolve_config()?;
    let root = PathBuf::from(&resolved.root);
    let (path_ref, suffix_line) = split_read_ref(reference);
    let target_line = explicit_line.or(suffix_line).unwrap_or(1).max(1);
    let path = resolve_read_path(&root, &PathBuf::from(&resolved.source_abs), &path_ref)?;
    let rel_path = relative_to(&path, &root)?;
    let content = read_text_file(&path)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return Ok(ok(json!(ReadResult {
            path: rel_path,
            target_line: 1,
            start_line: 1,
            end_line: 0,
            title: title_from_file(&root, &path),
            lines: Vec::new(),
        })));
    }

    let clamped_line = target_line.min(lines.len());
    let start_line = clamped_line.saturating_sub(context).max(1);
    let end_line = (clamped_line + context).min(lines.len());
    let selected = lines
        .iter()
        .enumerate()
        .skip(start_line - 1)
        .take(end_line - start_line + 1)
        .map(|(idx, text)| ReadLine {
            line: idx + 1,
            text: (*text).to_string(),
        })
        .collect();

    Ok(ok(json!(ReadResult {
        path: rel_path,
        target_line: clamped_line,
        start_line,
        end_line,
        title: title_from_file(&root, &path),
        lines: selected,
    })))
}
