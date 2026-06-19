use crate::domain::{ResolvedConfig, SyncItemResult};
use crate::support::*;
use serde::Deserialize;
use serde_json::json;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ConfluenceArchiveResponse {
    pub(crate) id: Option<String>,
}

pub(crate) fn runtime_archive_deleted(
    resolved: &ResolvedConfig,
    items: &mut [SyncItemResult],
) -> AppResult<bool> {
    let page_ids = items
        .iter()
        .filter(|item| item.action == "deleted" && item.platform_id.is_some())
        .filter_map(|item| item.platform_id.clone())
        .collect::<Vec<_>>();

    if page_ids.is_empty() {
        return Ok(false);
    }

    let base_url = resolved.target.base_url.as_deref().ok_or_else(|| {
        AppError::new(
            "CONFIG_MISSING_BASE_URL",
            format!(
                "run `conpub root <dir> --base-url <url>`, bind with --base-url, or set {ENV_BASE_URL}"
            ),
        )
    })?;
    let email = env::var("CONFLUENCE_EMAIL").map_err(|_| {
        AppError::new(
            "PUBLISH_CONFIG_ERROR",
            "CONFLUENCE_EMAIL is required to archive deleted pages",
        )
    })?;
    let api_key = env::var("CONFLUENCE_API_KEY")
        .or_else(|_| env::var("CONFLUENCE_API_TOKEN"))
        .map_err(|_| {
            AppError::new(
                "PUBLISH_CONFIG_ERROR",
                "CONFLUENCE_API_KEY is required to archive deleted pages",
            )
        })?;
    let runtime = build_async_runtime()?;
    let task_id = runtime.block_on(archive_deleted_pages(base_url, &email, &api_key, &page_ids))?;

    for item in items {
        if item.action == "deleted" && item.platform_id.is_some() {
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
