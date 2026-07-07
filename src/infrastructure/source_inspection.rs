use crate::domain::canonical_tags;
use crate::support::*;
use std::fs;
use std::path::Path;
use typub_core::ContentFormat;

pub(crate) struct InspectedDocument {
    pub(crate) title: String,
    pub(crate) tags: Vec<String>,
}

pub(crate) fn inspect_document_source(root: &Path, path: &Path) -> AppResult<InspectedDocument> {
    let text = fs::read_to_string(path).map_err(|err| {
        AppError::new(
            "FILE_READ_ERROR",
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    let format = match path.extension().and_then(|extension| extension.to_str()) {
        Some("md") => ContentFormat::Markdown,
        Some("typ") => ContentFormat::Typst,
        _ => {
            return Err(AppError::new(
                "UNSUPPORTED_DOCUMENT",
                format!("unsupported document source: {}", path.display()),
            ));
        }
    };

    match typub_engine::inspect_source(root, path, format) {
        Ok(inspection) => Ok(InspectedDocument {
            title: inspection
                .title
                .unwrap_or_else(|| fallback_title_from_path(path)),
            tags: canonical_tags(
                inspection.metadata.tags.unwrap_or_default(),
                &format!("source {}", path.display()),
            )?,
        }),
        Err(err) if declares_source_metadata(&text, format) => Err(AppError::new(
            "SOURCE_METADATA_ERROR",
            format!(
                "failed to inspect source metadata in {}: {err:#}",
                path.display()
            ),
        )),
        Err(_) => Ok(InspectedDocument {
            title: fallback_title_from_path(path),
            tags: Vec::new(),
        }),
    }
}

fn declares_source_metadata(text: &str, format: ContentFormat) -> bool {
    match format {
        ContentFormat::Markdown => has_markdown_frontmatter(text),
        ContentFormat::Typst => text.contains("<typub-meta>"),
    }
}

fn has_markdown_frontmatter(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(trim_frontmatter_line) != Some("---") {
        return false;
    }
    if lines
        .next()
        .is_none_or(|line| trim_frontmatter_line(line).is_empty())
    {
        return false;
    }
    lines.any(|line| matches!(trim_frontmatter_line(line), "---" | "..."))
}

fn trim_frontmatter_line(line: &str) -> &str {
    line.trim_end_matches([' ', '\t', '\r'])
}

fn fallback_title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("untitled")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_frontmatter_requires_a_complete_nonempty_block() {
        assert!(has_markdown_frontmatter(
            "---\ntags: [rust]\n---\n# Title\n"
        ));
        assert!(has_markdown_frontmatter(
            "---\ntags: [rust]\n...\n# Title\n"
        ));
        assert!(!has_markdown_frontmatter("---\n\nParagraph\n"));
        assert!(!has_markdown_frontmatter("---\nHorizontal rule only\n"));
    }
}
