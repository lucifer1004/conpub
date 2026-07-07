use crate::domain::*;
use crate::infrastructure::filesystem::{is_ignored, is_supported_doc};
use crate::support::*;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

const SHARED_ASSETS_DIR: &str = "_assets";
const STAGED_ASSETS_DIR: &str = "assets";

pub(crate) fn snapshot_documents(
    root: &Path,
    documents: &[Document],
) -> AppResult<Vec<DocumentSnapshot>> {
    let shared_assets_fingerprint = fingerprint_shared_assets(root)?;
    documents
        .iter()
        .map(|document| {
            Ok(DocumentSnapshot {
                document: document.clone(),
                slug: slug_for_path(&document.path),
                fingerprint: fingerprint_document(root, document, &shared_assets_fingerprint)?,
                parent_path: None,
                hierarchy_order: 0,
            })
        })
        .collect()
}

pub(crate) fn snapshot_hierarchy(
    root: &Path,
    entries: &[HierarchyEntry],
) -> AppResult<Vec<DocumentSnapshot>> {
    let shared_assets_fingerprint = fingerprint_shared_assets(root)?;
    entries
        .iter()
        .map(|entry| {
            Ok(DocumentSnapshot {
                document: entry.document.clone(),
                slug: slug_for_path(&entry.document.path),
                fingerprint: fingerprint_document(
                    root,
                    &entry.document,
                    &shared_assets_fingerprint,
                )?,
                parent_path: entry.parent_path.clone(),
                hierarchy_order: entry.order,
            })
        })
        .collect()
}

pub(crate) fn fingerprint_document(
    root: &Path,
    document: &Document,
    shared_assets_fingerprint: &str,
) -> AppResult<String> {
    let path = root.join(&document.path);
    let bytes = fs::read(&path).map_err(|err| {
        AppError::new(
            "FILE_READ_ERROR",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"conpub-document-v2\0");
    hasher.update(document.path.as_bytes());
    hasher.update(b"\0");
    hasher.update(&bytes);
    hasher.update(b"\0shared-assets\0");
    hasher.update(shared_assets_fingerprint.as_bytes());

    Ok(hasher.finalize().to_hex().to_string())
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentAsset {
    pub(crate) absolute: PathBuf,
    pub(crate) relative: String,
}

pub(crate) fn fingerprint_shared_assets(root: &Path) -> AppResult<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"conpub-shared-assets-v1\0");

    for asset in shared_asset_files(root)? {
        let asset_bytes = fs::read(&asset.absolute).map_err(|err| {
            AppError::new(
                "FILE_READ_ERROR",
                format!("failed to read {}: {err}", asset.absolute.display()),
            )
        })?;
        hasher.update(asset.relative.as_bytes());
        hasher.update(b"\0");
        hasher.update(&asset_bytes);
        hasher.update(b"\0");
    }

    Ok(hasher.finalize().to_hex().to_string())
}

pub(crate) fn shared_asset_files(root: &Path) -> AppResult<Vec<DocumentAsset>> {
    let assets_root = root.join(SHARED_ASSETS_DIR);
    if !assets_root.exists() {
        return Ok(Vec::new());
    }
    if !assets_root.is_dir() {
        return Err(AppError::new(
            "ASSETS_ROOT_NOT_DIRECTORY",
            format!(
                "shared assets path is not a directory: {}",
                assets_root.display()
            ),
        ));
    }

    let mut assets = Vec::new();

    for entry in WalkDir::new(&assets_root)
        .into_iter()
        .filter_entry(is_safe_asset_tree_entry)
    {
        let entry = entry.map_err(|err| AppError::new("STAGE_READ_ERROR", err.to_string()))?;
        if !entry.file_type().is_file() || !is_safe_publish_asset(entry.path()) {
            continue;
        }

        let relative = entry.path().strip_prefix(&assets_root).map_err(|_| {
            AppError::new(
                "PATH_OUTSIDE_ROOT",
                format!(
                    "asset is outside shared assets root: {}",
                    entry.path().display()
                ),
            )
        })?;
        assets.push(DocumentAsset {
            absolute: entry.path().to_path_buf(),
            relative: path_to_slash(relative),
        });
    }

    assets.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(assets)
}

fn is_safe_asset_tree_entry(entry: &DirEntry) -> bool {
    !is_ignored(entry)
        && entry
            .file_name()
            .to_str()
            .map(is_safe_asset_component)
            .unwrap_or(false)
}

fn is_safe_publish_asset(path: &Path) -> bool {
    !is_supported_doc(path)
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .map(is_safe_asset_component)
            .unwrap_or(false)
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "gif"
                        | "svg"
                        | "webp"
                        | "avif"
                        | "bmp"
                        | "ico"
                        | "pdf"
                )
            })
            .unwrap_or(false)
}

fn is_safe_asset_component(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.starts_with('.')
        && !matches!(
            name.as_str(),
            "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
                | "kubeconfig"
                | "credentials"
                | "credentials.json"
                | "service-account.json"
        )
}

pub(crate) fn stage_shared_assets(root: &Path, post_dir: &Path) -> AppResult<Vec<PathBuf>> {
    let assets = shared_asset_files(root)?;
    if assets.is_empty() {
        return Ok(Vec::new());
    }

    let assets_dir = post_dir.join(STAGED_ASSETS_DIR);
    fs::create_dir_all(&assets_dir).map_err(|err| {
        AppError::new(
            "STAGE_WRITE_ERROR",
            format!("failed to create {}: {err}", assets_dir.display()),
        )
    })?;

    let mut staged = Vec::new();
    for asset in assets {
        let destination = assets_dir.join(&asset.relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                AppError::new(
                    "STAGE_WRITE_ERROR",
                    format!("failed to create {}: {err}", parent.display()),
                )
            })?;
        }
        link_or_copy_path(&asset.absolute, &destination)?;
        staged.push(destination);
    }

    Ok(staged)
}

pub(crate) fn link_or_copy_path(source: &Path, destination: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(source, destination).is_ok() {
            return Ok(());
        }
    }

    copy_path(source, destination)
}

pub(crate) fn copy_path(source: &Path, destination: &Path) -> AppResult<()> {
    fs::copy(source, destination).map(|_| ()).map_err(|err| {
        AppError::new(
            "STAGE_WRITE_ERROR",
            format!(
                "failed to copy {} to {}: {err}",
                source.display(),
                destination.display()
            ),
        )
    })
}

/// Best-effort scan of document text for staged-asset references.
///
/// Documents reference shared assets as `assets/<name>`; staging provides
/// them from the root `_assets/` directory. Recognized forms: markdown
/// `](assets/...)` and quoted `"assets/..."` (typst `image("assets/...")`,
/// html `src="assets/..."`). Returned names are relative to `assets/`.
pub(crate) fn referenced_staged_assets(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (open, close) in [("](assets/", ')'), ("\"assets/", '"')] {
        let mut rest = text;
        while let Some(pos) = rest.find(open) {
            let tail = &rest[pos + open.len()..];
            let end = tail
                .find(|c: char| c == close || c.is_whitespace())
                .unwrap_or(tail.len());
            let name = &tail[..end];
            if !name.is_empty() && seen.insert(name.to_string()) {
                refs.push(name.to_string());
            }
            rest = &tail[end..];
        }
    }
    refs
}

/// Check every document's `assets/` references against the shared `_assets/`
/// directory. Returns `(document path, missing references)` pairs; an entry's
/// references are formatted as referenced (`assets/<name>`).
pub(crate) fn missing_staged_asset_references(
    root: &Path,
    documents: &[Document],
) -> AppResult<Vec<(String, Vec<String>)>> {
    let shared: std::collections::HashSet<String> = shared_asset_files(root)?
        .into_iter()
        .map(|asset| asset.relative)
        .collect();

    let mut missing = Vec::new();
    for document in documents {
        let path = root.join(&document.path);
        let text = fs::read_to_string(&path).map_err(|err| {
            AppError::new(
                "FILE_READ_ERROR",
                format!("failed to read {}: {err}", path.display()),
            )
        })?;
        let missing_refs: Vec<String> = referenced_staged_assets(&text)
            .into_iter()
            .filter(|reference| !shared.contains(reference))
            .map(|reference| format!("assets/{reference}"))
            .collect();
        if !missing_refs.is_empty() {
            missing.push((document.path.clone(), missing_refs));
        }
    }
    Ok(missing)
}

#[cfg(test)]
mod reference_tests {
    #![allow(clippy::expect_used)]
    use super::*;

    #[test]
    fn markdown_typst_and_html_references_are_found() {
        let text = r#"
![figure](assets/fig.png)
![titled](assets/plot.svg "a title")
#image("assets/diagram.png")
<img src="assets/photo.jpg">
not a ref: assets/loose.png and [link](https://example.com)
"#;
        let refs = referenced_staged_assets(text);
        assert_eq!(
            refs,
            vec!["fig.png", "plot.svg", "diagram.png", "photo.jpg"]
        );
    }

    #[test]
    fn missing_references_name_document_and_reference() {
        let dir = std::env::temp_dir().join("conpub-asset-ref-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("_assets")).expect("mkdir assets");
        fs::create_dir_all(dir.join("notes")).expect("mkdir notes");
        fs::write(dir.join("_assets/present.png"), b"png").expect("write asset");
        fs::write(
            dir.join("notes/doc.md"),
            "![ok](assets/present.png)\n![gone](assets/absent.png)\n",
        )
        .expect("write doc");

        let documents = vec![Document {
            path: "notes/doc.md".to_string(),
            title: "doc".to_string(),
            extension: "md".to_string(),
        }];
        let missing = missing_staged_asset_references(&dir, &documents).expect("scan");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].0, "notes/doc.md");
        assert_eq!(missing[0].1, vec!["assets/absent.png".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }
}
