use crate::support::*;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const FIRST_HEADING_EXPR: &str = r#"let plain-text(content) = {
  let fields = content.fields()
  if "text" in fields {
    fields.text
  } else if "children" in fields {
    fields.children.map(c => {
      if type(c) == str {
        c
      } else if c.func() == [ ].func() {
        " "
      } else {
        plain-text(c)
      }
    }).join()
  } else if "body" in fields {
    plain-text(fields.body)
  } else if "child" in fields {
    plain-text(fields.child)
  } else {
    ""
  }
}; let hs = query(heading); if hs.len() > 0 { plain-text(hs.first().body) } else { none }"#;

pub(crate) fn title_from_file(root: &Path, path: &Path) -> String {
    title_from_typst_introspection(root, path).unwrap_or_else(|| fallback_title_from_path(path))
}

fn title_from_typst_introspection(root: &Path, path: &Path) -> Option<String> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("md") => markdown_title_from_cmarker(root, path),
        Some("typ") => typst_title_from_heading(root, path),
        _ => None,
    }
}

fn markdown_title_from_cmarker(root: &Path, path: &Path) -> Option<String> {
    let root = canonical_or_original(root.to_path_buf());
    let input = typst_root_relative_path(&root, path)?;
    let wrapper = markdown_title_wrapper(&input);
    let root = root.to_str()?;
    let mut child = Command::new("typst")
        .args(["eval", "--root", root, "--in", "-", FIRST_HEADING_EXPR])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    child.stdin.as_mut()?.write_all(wrapper.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }

    parse_optional_title(&output.stdout)
}

fn typst_title_from_heading(root: &Path, path: &Path) -> Option<String> {
    let root = canonical_or_original(root.to_path_buf());
    let root = root.to_str()?;
    let path = path.to_str()?;
    let output = Command::new("typst")
        .args(["eval", "--root", root, "--in", path, FIRST_HEADING_EXPR])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    parse_optional_title(&output.stdout)
}

fn markdown_title_wrapper(input: &str) -> String {
    format!(
        r#"#import "@preview/cmarker:0.1.8"
#cmarker.render(
  read({input}),
  scope: (image: (source, alt: none, format: auto) => [],),
  math: (it, block: false) => [],
)
"#,
        input = typst_string(input)
    )
}

fn typst_root_relative_path(root: &Path, path: &Path) -> Option<String> {
    let path = canonical_or_original(path.to_path_buf());
    path.strip_prefix(root)
        .ok()
        .map(path_to_slash)
        .map(|path| format!("/{path}"))
}

fn typst_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn parse_optional_title(stdout: &[u8]) -> Option<String> {
    serde_json::from_slice::<Option<String>>(stdout)
        .ok()
        .flatten()
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty())
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
    fn parse_optional_title_rejects_empty_titles() {
        assert_eq!(
            parse_optional_title(br#""Title""#),
            Some("Title".to_string())
        );
        assert_eq!(parse_optional_title(br#""""#), None);
        assert_eq!(parse_optional_title(b"null"), None);
    }

    #[test]
    fn markdown_wrapper_escapes_paths() {
        let wrapper = markdown_title_wrapper("/docs/a \"quoted\" note.md");
        assert!(wrapper.contains(r#"read("/docs/a \"quoted\" note.md")"#));
    }
}
