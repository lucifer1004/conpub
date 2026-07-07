use crate::support::*;

pub(crate) fn canonical_tags(mut tags: Vec<String>, context: &str) -> AppResult<Vec<String>> {
    for tag in &tags {
        if !is_canonical_tag(tag) {
            return Err(AppError::new(
                "INVALID_TAG",
                format!(
                    "invalid tag `{tag}` in {context}; expected lowercase kebab-case up to 255 bytes"
                ),
            ));
        }
    }
    tags.sort();
    tags.dedup();
    Ok(tags)
}

fn is_canonical_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 255
        && tag.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_tags_are_sorted_and_deduplicated() -> AppResult<()> {
        assert_eq!(
            canonical_tags(
                vec![
                    "sm120".to_string(),
                    "inferlab".to_string(),
                    "sm120".to_string(),
                ],
                "test tags",
            )?,
            vec!["inferlab".to_string(), "sm120".to_string()]
        );
        Ok(())
    }

    #[test]
    fn non_canonical_tags_are_rejected() {
        for tag in ["Upper", "two words", "under_score", "-leading"] {
            assert!(canonical_tags(vec![tag.to_string()], "test tags").is_err());
        }
    }
}
