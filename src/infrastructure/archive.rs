use crate::domain::{ResolvedConfig, SyncItemResult};
use crate::infrastructure::config::resolve_confluence_credentials;
use crate::support::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ConfluenceArchiveResponse {
    pub(crate) id: Option<String>,
}

/// Deleted entries eligible for a remote action: superseded entries point at
/// a page owned by a live document and must never be touched remotely.
fn remote_eligible(item: &SyncItemResult) -> bool {
    item.action == "deleted" && item.status != "superseded" && item.platform_id.is_some()
}

fn resolved_base_url(resolved: &ResolvedConfig) -> AppResult<&str> {
    resolved.target.base_url.as_deref().ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_BASE_URL",
            format!(
                "run `conpub root <dir> --base-url <url>`, bind with --base-url, or set {ENV_BASE_URL}"
            ),
        )
    })
}

/// Resolve write credentials through the same env > project > user chain the
/// publish path uses, reporting every missing field with its lookup chain.
fn confluence_write_credentials(action: &str) -> AppResult<(String, String)> {
    let credentials = resolve_confluence_credentials()?;
    match (credentials.email, credentials.api_key) {
        (Some(email), Some(api_key)) => Ok((email, api_key)),
        (email, api_key) => {
            let mut missing = Vec::new();
            if api_key.is_none() {
                missing.push(format!(
                    "api_key ([confluence] api_key or {ENV_CONFLUENCE_API_KEY})"
                ));
            }
            if email.is_none() {
                missing.push(format!(
                    "email ([confluence] email or {ENV_CONFLUENCE_EMAIL})"
                ));
            }
            Err(AppError::new(
                "PUBLISH_CONFIG_ERROR",
                format!(
                    "Confluence credentials missing to {action}: {}",
                    missing.join("; ")
                ),
            ))
        }
    }
}

pub(crate) fn runtime_archive_deleted(
    resolved: &ResolvedConfig,
    items: &mut [SyncItemResult],
) -> AppResult<bool> {
    let page_ids = items
        .iter()
        .filter(|item| remote_eligible(item))
        .filter_map(|item| item.platform_id.clone())
        .collect::<Vec<_>>();

    if page_ids.is_empty() {
        return Ok(false);
    }

    let base_url = resolved_base_url(resolved)?;
    let (email, api_key) = confluence_write_credentials("archive deleted pages")?;
    let runtime = build_async_runtime()?;
    let task_id = runtime.block_on(archive_deleted_pages(base_url, &email, &api_key, &page_ids))?;

    for item in items {
        if remote_eligible(item) {
            item.status = "archived".to_string();
            item.archive_task_id = task_id.clone();
            item.reason = Some("archive request accepted by Confluence".to_string());
        }
    }

    Ok(true)
}

pub(crate) async fn archive_deleted_pages(
    base_url: &str,
    email: &str,
    api_key: &str,
    page_ids: &[String],
) -> AppResult<Option<String>> {
    let base_url = normalize_confluence_base_url(base_url);
    let url = format!("{base_url}/wiki/rest/api/content/archive");
    let pages = page_ids
        .iter()
        .map(|id| {
            let id = id
                .parse::<u64>()
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| serde_json::Value::String(id.clone()));
            json!({ "id": id })
        })
        .collect::<Vec<_>>();
    let response = reqwest::Client::new()
        .post(url)
        .basic_auth(email, Some(api_key))
        .header("Accept", "application/json")
        .json(&json!({ "pages": pages }))
        .send()
        .await
        .map_err(|err| AppError::new("ARCHIVE_REQUEST_ERROR", err.to_string()))?;
    let status = response.status();
    if status.as_u16() != 202 {
        return Err(AppError::new(
            "ARCHIVE_REQUEST_FAILED",
            format!("Confluence archive request returned {status}"),
        ));
    }

    let text = response
        .text()
        .await
        .map_err(|err| AppError::new("ARCHIVE_RESPONSE_ERROR", err.to_string()))?;

    let parsed = serde_json::from_str::<ConfluenceArchiveResponse>(&text).ok();
    Ok(parsed.and_then(|body| body.id))
}

/// Permanently delete the Confluence pages of eligible deleted entries.
/// Per-page results: a 404 counts as success so a rerun after a partial
/// failure is idempotent. Response bodies are never surfaced (same leak
/// hardening as the archive path).
pub(crate) fn runtime_delete_deleted(
    resolved: &ResolvedConfig,
    items: &mut [SyncItemResult],
) -> AppResult<bool> {
    let page_ids = items
        .iter()
        .filter(|item| remote_eligible(item))
        .filter_map(|item| item.platform_id.clone())
        .collect::<Vec<_>>();

    if page_ids.is_empty() {
        return Ok(false);
    }

    let base_url = resolved_base_url(resolved)?;
    let (email, api_key) = confluence_write_credentials("delete pages")?;
    let runtime = build_async_runtime()?;
    let results = runtime.block_on(delete_pages(base_url, &email, &api_key, &page_ids))?;
    let by_id = results
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();

    for item in items {
        if !remote_eligible(item) {
            continue;
        }
        let Some(id) = item.platform_id.as_ref() else {
            continue;
        };
        match by_id.get(id) {
            Some(Ok(())) => {
                item.status = "deleted_remote".to_string();
                item.reason = Some("Confluence page deleted".to_string());
            }
            Some(Err(message)) => {
                item.status = "failed".to_string();
                item.error = Some(message.clone());
            }
            None => {}
        }
    }

    Ok(true)
}

type DeleteOutcomes = Vec<(String, Result<(), String>)>;

pub(crate) async fn delete_pages(
    base_url: &str,
    email: &str,
    api_key: &str,
    page_ids: &[String],
) -> AppResult<DeleteOutcomes> {
    let base_url = normalize_confluence_base_url(base_url);
    let client = reqwest::Client::new();
    let mut outcomes = Vec::with_capacity(page_ids.len());
    for id in page_ids {
        let url = format!("{base_url}/wiki/rest/api/content/{id}");
        let outcome = match client
            .delete(&url)
            .basic_auth(email, Some(api_key))
            .send()
            .await
        {
            Err(err) => Err(format!("delete request error: {err}")),
            Ok(response) => match response.status().as_u16() {
                200 | 202 | 204 => Ok(()),
                // Already gone: a rerun after a partial failure stays clean.
                404 => Ok(()),
                status => Err(format!("Confluence delete returned {status}")),
            },
        };
        outcomes.push((id.clone(), outcome));
    }
    Ok(outcomes)
}
