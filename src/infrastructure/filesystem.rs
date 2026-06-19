use super::title::title_from_file;
use crate::domain::Document;
use crate::support::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

pub(crate) fn list_documents(root: &Path, scope: &Path) -> AppResult<Vec<Document>> {
    let mut docs = Vec::new();

    if !scope.exists() {
        return Err(AppError::new(
            "SOURCE_NOT_FOUND",
            format!("source does not exist: {}", scope.display()),
        ));
    }

    for entry in WalkDir::new(scope)
        .into_iter()
        .filter_entry(|entry| !is_ignored(entry))
    {
        let entry = entry.map_err(|err| AppError::new("WALK_ERROR", err.to_string()))?;
        if !entry.file_type().is_file() || !is_supported_doc(entry.path()) {
            continue;
        }

        let rel_path = relative_to(entry.path(), root)?;
        let extension = entry
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_string();

        docs.push(Document {
            title: title_from_file(root, entry.path()),
            path: rel_path,
            extension,
        });
    }

    docs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(docs)
}

pub(crate) fn is_ignored(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|name| {
            name == ".git"
                || name == "node_modules"
                || name == ".cache"
                || (name.starts_with('.') && entry.file_type().is_dir())
        })
        .unwrap_or(false)
}

pub(crate) fn is_supported_doc(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

pub(crate) fn resolve_document_subset(
    root: &Path,
    source: &Path,
    references: &[String],
) -> AppResult<Vec<Document>> {
    let mut by_path = HashMap::new();

    for reference in references {
        let (path_ref, _) = split_read_ref(reference);
        let path = resolve_read_path(root, source, &path_ref)?;
        let path = canonical_or_original(path);

        if !path.starts_with(source) {
            return Err(AppError::new(
                "PATH_OUTSIDE_SOURCE",
                format!("sync path is outside bound source: {path_ref}"),
            ));
        }

        if path.is_dir() {
            for document in list_documents(root, &path)? {
                by_path.insert(document.path.clone(), document);
            }
        } else if path.is_file() && is_supported_doc(&path) {
            let extension = path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_string();
            let document = Document {
                path: relative_to(&path, root)?,
                title: title_from_file(root, &path),
                extension,
            };
            by_path.insert(document.path.clone(), document);
        } else {
            return Err(AppError::new(
                "UNSUPPORTED_DOCUMENT",
                format!("sync path is not a supported Markdown or Typst document: {path_ref}"),
            ));
        }
    }

    let mut documents = by_path.into_values().collect::<Vec<_>>();
    documents.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(documents)
}

pub(crate) fn read_text_file(path: &Path) -> AppResult<String> {
    fs::read_to_string(path).map_err(|err| {
        AppError::new(
            "FILE_READ_ERROR",
            format!("failed to read {}: {err}", path.display()),
        )
    })
}

pub(crate) fn split_read_ref(reference: &str) -> (String, Option<usize>) {
    if let Some((path, line)) = reference.rsplit_once(':')
        && let Ok(line_number) = line.parse::<usize>()
    {
        return (path.to_string(), Some(line_number));
    }

    (reference.to_string(), None)
}

pub(crate) fn resolve_read_path(root: &Path, source: &Path, path_ref: &str) -> AppResult<PathBuf> {
    let root_relative = root.join(path_ref);
    if root_relative.exists() {
        return Ok(root_relative);
    }

    let source_relative = source.join(path_ref);
    if source_relative.exists() {
        return Ok(source_relative);
    }

    Err(AppError::new(
        "FILE_NOT_FOUND",
        format!("file not found under root or source: {path_ref}"),
    ))
}

pub(crate) fn relative_to(path: &Path, root: &Path) -> AppResult<String> {
    let path_abs = canonical_or_original(path.to_path_buf());
    let root_abs = canonical_or_original(root.to_path_buf());
    let rel = path_abs.strip_prefix(&root_abs).map_err(|_| {
        AppError::new(
            "PATH_OUTSIDE_ROOT",
            format!("path is outside configured root: {}", path.display()),
        )
    })?;
    Ok(path_to_slash(rel))
}
