//! 四家搜索提供商的上游客户端（搜索 + 抓取）。
//! 密钥选择、限额状态与故障切换由共享请求层处理；批量抓取只重试失败的请求。

use serde_json::{json, Value};

use super::provider_request::ProviderRequest;

use super::normalize::{
    draft_source, text_of, SourceDraft, MAX_CONTENT_LENGTH, MAX_FETCHED_CONTENT_LENGTH,
};
use super::search::{FetchErrorItem, FetchPageDraft, SearchError, SearchErrorCode};

const TAVILY_SEARCH: &str = "https://api.tavily.com/search";
const TAVILY_EXTRACT: &str = "https://api.tavily.com/extract";
const QUERIT_SEARCH: &str = "https://api.querit.ai/v1/search";
const QUERIT_CONTENTS: &str = "https://api.querit.ai/v1/contents";
const FIRECRAWL_SEARCH: &str = "https://api.firecrawl.dev/v2/search";
const FIRECRAWL_SCRAPE: &str = "https://api.firecrawl.dev/v2/scrape";
const OLLAMA_SEARCH: &str = "https://ollama.com/api/web_search";
const OLLAMA_FETCH: &str = "https://ollama.com/api/web_fetch";

pub fn display_name(provider: &str) -> &'static str {
    match provider {
        "tavily" => "Tavily",
        "querit" => "Querit",
        "firecrawl" => "Firecrawl",
        "ollama" => "Ollama Web Search",
        _ => "",
    }
}

fn array_of<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn nested<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut node = value;
    for key in keys {
        node = node.get(*key)?;
    }
    Some(node)
}

/* ─── Tavily ───────────────────────────────────────────────── */

async fn tavily_search(request: &ProviderRequest<'_>, query: &str) -> Result<Vec<SourceDraft>, SearchError> {
    let body = request.post_json(
        TAVILY_SEARCH,
        json!({
            "query": query,
            "topic": "general",
            "search_depth": "basic",
            "max_results": 10,
            "include_answer": false,
            "include_raw_content": false,
            "include_images": false,
        }),
    )
    .await?;
    Ok(array_of(&body, "results")
        .iter()
        .filter_map(|item| {
            draft_source(
                display_name("tavily"),
                item.get("url").unwrap_or(&Value::Null),
                item.get("title").unwrap_or(&Value::Null),
                item.get("content").unwrap_or(&Value::Null),
                item.get("published_date").unwrap_or(&Value::Null),
                MAX_CONTENT_LENGTH,
            )
        })
        .collect())
}

async fn tavily_fetch(
    request: &ProviderRequest<'_>,
    urls: &[String],
) -> Result<(Vec<FetchPageDraft>, Vec<FetchErrorItem>), SearchError> {
    let body = request.post_json(
        TAVILY_EXTRACT,
        json!({ "urls": urls, "extract_depth": "basic", "include_images": false, "format": "markdown" }),
    )
    .await?;
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    for item in array_of(&body, "results") {
        let requested_url = text_of(item, "url");
        match draft_source(
            display_name("tavily"),
            item.get("url").unwrap_or(&Value::Null),
            item.get("title").unwrap_or(&Value::Null),
            item.get("raw_content").unwrap_or(&Value::Null),
            item.get("published_date").unwrap_or(&Value::Null),
            MAX_FETCHED_CONTENT_LENGTH,
        ) {
            Some(source) => pages.push(FetchPageDraft::from_source(requested_url, source)),
            None if !requested_url.is_empty() => errors.push(FetchErrorItem {
                url: requested_url,
                code: SearchErrorCode::ProviderUnavailable,
                message: "The page returned no readable content".into(),
            }),
            None => {}
        }
    }
    for failed in array_of(&body, "failed_results") {
        let url = text_of(failed, "url");
        if url.is_empty() {
            continue;
        }
        let reason = text_of(failed, "error");
        errors.push(FetchErrorItem {
            url,
            code: SearchErrorCode::ProviderUnavailable,
            message: if reason.is_empty() {
                "Tavily failed to extract this page".into()
            } else {
                format!("Tavily failed to extract this page: {}", reason.chars().take(200).collect::<String>())
            },
        });
    }
    if pages.is_empty() && errors.is_empty() {
        return Err(SearchError::new(SearchErrorCode::NoResults, "The provider returned no readable content"));
    }
    Ok((pages, errors))
}

/* ─── Querit ───────────────────────────────────────────────── */

fn querit_published_date(item: &Value) -> String {
    let age = text_of(item, "page_age");
    if !age.is_empty() {
        return age;
    }
    let Some(time) = item.get("page_time").and_then(Value::as_f64) else {
        return String::new();
    };
    if !time.is_finite() || time <= 0.0 {
        return String::new();
    }
    let millis = if time >= 1_000_000_000_000.0 { time } else { time * 1000.0 };
    use chrono::{TimeZone, Utc};
    Utc.timestamp_millis_opt(millis as i64)
        .single()
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

async fn querit_search(request: &ProviderRequest<'_>, query: &str) -> Result<Vec<SourceDraft>, SearchError> {
    let body = request.post_json(
        QUERIT_SEARCH,
        json!({ "query": query, "count": 10 }),
    )
    .await?;
    let results = nested(&body, &["results", "result"]).and_then(Value::as_array);
    Ok(results
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            draft_source(
                display_name("querit"),
                item.get("url").unwrap_or(&Value::Null),
                item.get("title").unwrap_or(&Value::Null),
                item.get("snippet").unwrap_or(&Value::Null),
                &json!(querit_published_date(item)),
                MAX_CONTENT_LENGTH,
            )
        })
        .collect())
}

async fn querit_fetch(
    request: &ProviderRequest<'_>,
    urls: &[String],
) -> Result<(Vec<FetchPageDraft>, Vec<FetchErrorItem>), SearchError> {
    let body = request.post_json(
        QUERIT_CONTENTS,
        json!({ "urls": urls, "format": "markdown", "crawlTimeout": 30, "extrasMeta": true }),
    )
    .await?;
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    for item in array_of(&body, "results") {
        let requested_url = text_of(item, "url");
        let meta = item.get("extrasMeta").filter(|value| value.is_object());
        let date = meta
            .and_then(|value| value.get("publishTime"))
            .or_else(|| item.get("published_date"))
            .unwrap_or(&Value::Null);
        let effective_url = if requested_url.is_empty() {
            meta.and_then(|value| value.get("url")).and_then(Value::as_str).unwrap_or("").to_string()
        } else {
            requested_url.clone()
        };
        match draft_source(
            display_name("querit"),
            &json!(effective_url),
            meta.and_then(|value| value.get("title")).or_else(|| item.get("title")).unwrap_or(&Value::Null),
            item.get("content").unwrap_or(&Value::Null),
            date,
            MAX_FETCHED_CONTENT_LENGTH,
        ) {
            Some(source) => pages.push(FetchPageDraft::from_source(requested_url, source)),
            None if !requested_url.is_empty() => errors.push(FetchErrorItem {
                url: requested_url,
                code: SearchErrorCode::ProviderUnavailable,
                message: "The page returned no readable content".into(),
            }),
            None => {}
        }
    }
    if pages.is_empty() && errors.is_empty() {
        return Err(SearchError::new(SearchErrorCode::NoResults, "The provider returned no readable content"));
    }
    Ok((pages, errors))
}

/* ─── Firecrawl ────────────────────────────────────────────── */

async fn firecrawl_search(request: &ProviderRequest<'_>, query: &str) -> Result<Vec<SourceDraft>, SearchError> {
    let body = request.post_json(
        FIRECRAWL_SEARCH,
        json!({ "query": query, "limit": 10, "sources": ["web"] }),
    )
    .await?;
    let results = nested(&body, &["data", "web"]).and_then(Value::as_array);
    Ok(results
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| {
            let meta = item.get("metadata").filter(|value| value.is_object());
            let content = {
                let description = text_of(item, "description");
                if description.is_empty() {
                    text_of(item, "markdown")
                } else {
                    description
                }
            };
            let date = {
                let direct = text_of(item, "date");
                if direct.is_empty() {
                    meta.and_then(|value| value.get("publishedTime")).cloned().unwrap_or(Value::Null)
                } else {
                    json!(direct)
                }
            };
            draft_source(
                display_name("firecrawl"),
                item.get("url").unwrap_or(&Value::Null),
                item.get("title").unwrap_or(&Value::Null),
                &json!(content),
                &date,
                MAX_CONTENT_LENGTH,
            )
        })
        .collect())
}

async fn firecrawl_scrape_one(
    request: &ProviderRequest<'_>,
    url: &str,
) -> Result<FetchPageDraft, FetchErrorItem> {
    let body = request.post_json(
        FIRECRAWL_SCRAPE,
        json!({ "url": url, "formats": ["markdown"], "onlyMainContent": true, "removeBase64Images": true }),
    )
    .await
    .map_err(|error| FetchErrorItem {
        url: url.to_string(),
        code: error.code,
        message: error.message,
    })?;
    let data = body.get("data").filter(|value| value.is_object());
    let meta = data.and_then(|value| value.get("metadata")).filter(|value| value.is_object());
    let fetched_url = meta
        .and_then(|value| value.get("sourceURL").or_else(|| value.get("url")))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(url);
    let date = meta
        .and_then(|value| value.get("publishedTime").or_else(|| value.get("publishedDate")))
        .cloned()
        .unwrap_or(Value::Null);
    match draft_source(
        display_name("firecrawl"),
        &json!(fetched_url),
        meta.and_then(|value| value.get("title")).unwrap_or(&Value::Null),
        data.and_then(|value| value.get("markdown")).unwrap_or(&Value::Null),
        &date,
        MAX_FETCHED_CONTENT_LENGTH,
    ) {
        Some(source) => Ok(FetchPageDraft::from_source(url.to_string(), source)),
        None => Err(FetchErrorItem {
            url: url.to_string(),
            code: SearchErrorCode::ProviderUnavailable,
            message: "The page returned no readable content".into(),
        }),
    }
}

async fn firecrawl_fetch(
    request: &ProviderRequest<'_>,
    urls: &[String],
) -> Result<(Vec<FetchPageDraft>, Vec<FetchErrorItem>), SearchError> {
    let outcomes = futures::future::join_all(
        urls.iter().map(|url| firecrawl_scrape_one(request, url)),
    )
    .await;
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(page) => pages.push(page),
            Err(error) => errors.push(error),
        }
    }
    if pages.is_empty() && errors.is_empty() {
        return Err(SearchError::new(SearchErrorCode::NoResults, "The provider returned no readable content"));
    }
    Ok((pages, errors))
}

/* ─── Ollama Web Search ────────────────────────────────────── */

async fn ollama_search(request: &ProviderRequest<'_>, query: &str) -> Result<Vec<SourceDraft>, SearchError> {
    let body = request.post_json(
        OLLAMA_SEARCH,
        json!({ "query": query, "max_results": 10 }),
    )
    .await?;
    Ok(array_of(&body, "results")
        .iter()
        .filter_map(|item| {
            let date = item
                .get("published_date")
                .or_else(|| item.get("publishedDate"))
                .cloned()
                .unwrap_or(Value::Null);
            draft_source(
                display_name("ollama"),
                item.get("url").unwrap_or(&Value::Null),
                item.get("title").unwrap_or(&Value::Null),
                item.get("content").unwrap_or(&Value::Null),
                &date,
                MAX_CONTENT_LENGTH,
            )
        })
        .collect())
}

async fn ollama_fetch_one(request: &ProviderRequest<'_>, url: &str) -> Result<FetchPageDraft, FetchErrorItem> {
    let body = request.post_json(OLLAMA_FETCH, json!({ "url": url }))
        .await
        .map_err(|error| FetchErrorItem {
            url: url.to_string(),
            code: error.code,
            message: error.message,
        })?;
    let date = body
        .get("published_date")
        .or_else(|| body.get("publishedDate"))
        .cloned()
        .unwrap_or(Value::Null);
    match draft_source(
        display_name("ollama"),
        &json!(url),
        body.get("title").unwrap_or(&Value::Null),
        body.get("content").unwrap_or(&Value::Null),
        &date,
        MAX_FETCHED_CONTENT_LENGTH,
    ) {
        Some(source) => Ok(FetchPageDraft::from_source(url.to_string(), source)),
        None => Err(FetchErrorItem {
            url: url.to_string(),
            code: SearchErrorCode::ProviderUnavailable,
            message: "The page returned no readable content".into(),
        }),
    }
}

async fn ollama_fetch(
    request: &ProviderRequest<'_>,
    urls: &[String],
) -> Result<(Vec<FetchPageDraft>, Vec<FetchErrorItem>), SearchError> {
    let outcomes = futures::future::join_all(
        urls.iter().map(|url| ollama_fetch_one(request, url)),
    )
    .await;
    let mut pages = Vec::new();
    let mut errors = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(page) => pages.push(page),
            Err(error) => errors.push(error),
        }
    }
    if pages.is_empty() && errors.is_empty() {
        return Err(SearchError::new(SearchErrorCode::NoResults, "The provider returned no readable content"));
    }
    Ok((pages, errors))
}

/* ─── 分发 ─────────────────────────────────────────────────── */

pub async fn search(provider: &str, query: &str, timeout: u64) -> Result<Vec<SourceDraft>, SearchError> {
    search_with_request(provider, &ProviderRequest::new(provider, "search", timeout), query).await
}

async fn search_with_request(provider: &str, request: &ProviderRequest<'_>, query: &str) -> Result<Vec<SourceDraft>, SearchError> {
    match provider {
        "tavily" => tavily_search(request, query).await,
        "querit" => querit_search(request, query).await,
        "firecrawl" => firecrawl_search(request, query).await,
        "ollama" => ollama_search(request, query).await,
        _ => Err(SearchError::new(SearchErrorCode::ProviderUnavailable, "Unknown search provider")),
    }
}

pub async fn fetch(provider: &str, urls: &[String], timeout: u64) -> Result<(Vec<FetchPageDraft>, Vec<FetchErrorItem>), SearchError> {
    let request = ProviderRequest::new(provider, "fetch", timeout);
    match provider {
        "tavily" => tavily_fetch(&request, urls).await,
        "querit" => querit_fetch(&request, urls).await,
        "firecrawl" => firecrawl_fetch(&request, urls).await,
        "ollama" => ollama_fetch(&request, urls).await,
        _ => Err(SearchError::new(SearchErrorCode::ProviderUnavailable, "Unknown search provider")),
    }
}

/// 管理端「测试连接」可检查指定密钥，即使它尚在额度冷却期。
pub async fn test_provider(provider: &str, key_id: Option<&str>, operation: &str) -> Value {
    let started = crate::server::logging::now_ms();
    let duration = || (crate::server::logging::now_ms() - started) as u64;
    let timeout = super::timeout_seconds(super::WEB_SEARCH_SERVICE_ID);
    let request = ProviderRequest::for_test(provider, if operation == "fetch" { "fetch" } else { "search" }, timeout, key_id);
    let outcome = if operation == "fetch" {
        request.post_json(match provider {
            "tavily" => TAVILY_EXTRACT,
            "querit" => QUERIT_CONTENTS,
            "firecrawl" => FIRECRAWL_SCRAPE,
            _ => OLLAMA_FETCH,
        }, match provider {
            "tavily" => json!({ "urls": ["https://example.com"], "extract_depth": "basic" }),
            "querit" => json!({ "urls": ["https://example.com"], "format": "markdown" }),
            "firecrawl" => json!({ "url": "https://example.com", "formats": ["markdown"] }),
            _ => json!({ "url": "https://example.com" }),
        }).await.map(|_| 1)
    } else {
        search_with_request(provider, &request, "Agent2API connectivity check").await
            .and_then(|sources| if sources.is_empty() {
                Err(SearchError::new(SearchErrorCode::NoResults, "The search provider returned no results"))
            } else { Ok(sources.len()) })
    };
    match outcome {
        Ok(count) => json!({
            "success": true, "provider": provider, "key_id": key_id,
            "operation": operation, "result_count": count, "duration_ms": duration(),
        }),
        Err(error) => json!({
            "success": false, "provider": provider, "key_id": key_id, "operation": operation,
            "code": error.code.as_str(), "error": error.message, "reset_at": error.reset_at,
            "duration_ms": duration(),
        }),
    }
}
