use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{AppError, AppResult};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

/// Web 工具共享 HTTP client，适用于搜索和网页抓取复用连接池。
static WEB_HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

/// 默认搜索结果数量。
const DEFAULT_SEARCH_RESULTS: u32 = 8;
/// 最大搜索结果数量，避免工具输出过大。
const MAX_SEARCH_RESULTS: u32 = 20;
/// 默认搜索上下文字符数。
const DEFAULT_SEARCH_CONTEXT_CHARS: usize = 10_000;
/// 最大搜索上下文字符数。
const MAX_SEARCH_CONTEXT_CHARS: usize = 40_000;
/// 搜索请求超时。
const SEARCH_TIMEOUT: Duration = Duration::from_secs(25);
/// 单次网页抓取最大字节数。
const MAX_FETCH_BYTES: usize = 5 * 1024 * 1024;
/// 默认网页抓取超时。
const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 30;
/// 最大网页抓取超时。
const MAX_FETCH_TIMEOUT_SECS: u64 = 120;
/// 返回给模型的单次抓取最大字符数。
const MAX_FETCH_OUTPUT_CHARS: usize = 120_000;
/// Reader 提取内容最小有效字符数。
const MIN_READER_CONTENT_CHARS: usize = 80;
/// 搜索服务地址。
const SEARCH_ENDPOINT: &str = "https://mcp.exa.ai/mcp";
/// Reader 服务地址前缀。
const JINA_READER_PREFIX: &str = "https://r.jina.ai/";

/// websearch 工具参数。
#[derive(Debug, Clone, Deserialize)]
struct WebSearchArgs {
    /// 搜索查询。
    query: String,
    /// 返回结果数量。
    num_results: Option<u32>,
    /// 搜索类型。
    search_type: Option<String>,
    /// 实时抓取策略。
    livecrawl: Option<String>,
    /// 搜索上下文最大字符数。
    context_max_chars: Option<usize>,
}

/// webfetch 工具参数。
#[derive(Debug, Clone, Deserialize)]
struct WebFetchArgs {
    /// 需要抓取的 URL。
    url: String,
    /// 返回格式：markdown、text 或 html。
    format: Option<String>,
    /// 请求超时秒数。
    timeout: Option<u64>,
}

/// MCP JSON-RPC 请求。
#[derive(Debug, Serialize)]
struct McpToolCall<'a, T> {
    /// JSON-RPC 版本。
    jsonrpc: &'static str,
    /// 固定请求 id。
    id: u64,
    /// MCP 方法名。
    method: &'static str,
    /// MCP 参数。
    params: McpToolCallParams<'a, T>,
}

/// MCP tools/call 参数。
#[derive(Debug, Serialize)]
struct McpToolCallParams<'a, T> {
    /// 后端工具名。
    name: &'a str,
    /// 后端工具参数。
    arguments: T,
}

/// 搜索后端参数。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchBackendArgs {
    /// 搜索查询。
    query: String,
    /// 搜索类型。
    r#type: String,
    /// 返回结果数量。
    num_results: u32,
    /// 实时抓取策略。
    livecrawl: String,
    /// 搜索上下文最大字符数。
    context_max_characters: usize,
}

/// 网页抓取来源。
#[derive(Debug, Clone, Copy)]
enum FetchSource {
    /// Reader 服务提取的正文。
    Reader,
    /// 直接请求原始 URL 后本地提取。
    Direct,
}

/// 网页抓取结果。
#[derive(Debug, Clone)]
struct FetchPageContent {
    /// 返回给模型的正文。
    body: String,
    /// HTTP Content-Type。
    content_type: String,
    /// 实际使用的抓取来源。
    source: FetchSource,
    /// Reader 失败原因，只有 fallback 到 direct 时存在。
    reader_error: Option<String>,
}

/// websearch 工具。
pub struct WebSearchHandler;

/// webfetch 工具。
pub struct WebFetchHandler;

#[async_trait]
impl ToolHandler for WebSearchHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "websearch"
    }

    /// 返回 provider 无关 websearch 工具 spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "query".to_string(),
                JsonSchema::string(Some(
                    "Search query. Include the current year when looking for recent information."
                        .to_string(),
                )),
            ),
            (
                "num_results".to_string(),
                JsonSchema::integer(Some(
                    "Number of search results to return. Defaults to 8, max 20.".to_string(),
                )),
            ),
            (
                "search_type".to_string(),
                JsonSchema::string_enum(
                    vec![json!("auto"), json!("fast"), json!("deep")],
                    Some("Search depth: auto, fast, or deep.".to_string()),
                ),
            ),
            (
                "livecrawl".to_string(),
                JsonSchema::string_enum(
                    vec![json!("fallback"), json!("preferred")],
                    Some("Live crawling mode when available.".to_string()),
                ),
            ),
            (
                "context_max_chars".to_string(),
                JsonSchema::integer(Some(
                    "Maximum characters of search context. Defaults to 10000.".to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Search the web for current information, recent events, and relevant sources. Use webfetch when you already have a specific URL to read.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["query".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 执行 websearch 并返回搜索结果文本。
    async fn execute(&self, call: ToolCall, _context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "websearch requires function arguments".to_string(),
            ));
        };
        let args: WebSearchArgs = serde_json::from_str(arguments)?;
        let output = execute_websearch(args).await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: Value::String(output),
        })
    }
}

#[async_trait]
impl ToolHandler for WebFetchHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "webfetch"
    }

    /// 返回 provider 无关 webfetch 工具 spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "url".to_string(),
                JsonSchema::string(Some("HTTP or HTTPS URL to fetch and read.".to_string())),
            ),
            (
                "format".to_string(),
                JsonSchema::string_enum(
                    vec![json!("markdown"), json!("text"), json!("html")],
                    Some("Output format. Defaults to markdown.".to_string()),
                ),
            ),
            (
                "timeout".to_string(),
                JsonSchema::integer(Some(
                    "Optional timeout in seconds. Defaults to 30, max 120.".to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Fetch a specific URL and return readable page content. Use websearch first when you need to discover sources.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["url".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 执行 webfetch 并返回网页内容。
    async fn execute(&self, call: ToolCall, _context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "webfetch requires function arguments".to_string(),
            ));
        };
        let args: WebFetchArgs = serde_json::from_str(arguments)?;
        let output = execute_webfetch(args).await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: Value::String(output),
        })
    }
}

/// 执行搜索后端调用，适用于模型通过 websearch 获取实时资料。
async fn execute_websearch(args: WebSearchArgs) -> AppResult<String> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(AppError::Tool("websearch query is empty".to_string()));
    }
    let search_type = normalized_choice(
        args.search_type.as_deref(),
        &["auto", "fast", "deep"],
        "auto",
        "search_type",
    )?;
    let livecrawl = normalized_choice(
        args.livecrawl.as_deref(),
        &["fallback", "preferred"],
        "fallback",
        "livecrawl",
    )?;
    let num_results = args
        .num_results
        .unwrap_or(DEFAULT_SEARCH_RESULTS)
        .clamp(1, MAX_SEARCH_RESULTS);
    let context_max_characters = args
        .context_max_chars
        .unwrap_or(DEFAULT_SEARCH_CONTEXT_CHARS)
        .clamp(1_000, MAX_SEARCH_CONTEXT_CHARS);
    let backend_args = SearchBackendArgs {
        query: query.to_string(),
        r#type: search_type.to_string(),
        num_results,
        livecrawl: livecrawl.to_string(),
        context_max_characters,
    };
    let request = McpToolCall {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: McpToolCallParams {
            // 为什么需要它:
            // 触发条件: 模型调用统一的 websearch 工具。
            // 不能直接用常规路径的原因: 后端服务要求自己的工具名。
            // 防止副作用: 避免把后端名暴露到 provider tool spec。
            name: "web_search_exa",
            arguments: backend_args,
        },
    };
    let response = tokio::time::timeout(SEARCH_TIMEOUT, async {
        web_http_client()?
            .post(search_endpoint())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await
            .map_err(AppError::from)
    })
    .await
    .map_err(|_| AppError::Tool("websearch request timed out".to_string()))??;
    if !response.status().is_success() {
        return Err(AppError::Tool(format!(
            "websearch request failed with status {}",
            response.status()
        )));
    }
    let body = response.text().await?;
    let parsed = parse_search_response(&body)?;
    let truncated = truncate_chars(parsed.trim(), context_max_characters);
    Ok(format!("Search results for: {query}\n\n{truncated}"))
}

/// 执行 URL 抓取，适用于模型已经有明确网页地址的场景。
async fn execute_webfetch(args: WebFetchArgs) -> AppResult<String> {
    let url = args.url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::Tool(
            "webfetch url must start with http:// or https://".to_string(),
        ));
    }
    let format = normalized_choice(
        args.format.as_deref(),
        &["markdown", "text", "html"],
        "markdown",
        "format",
    )?;
    let timeout_secs = args
        .timeout
        .unwrap_or(DEFAULT_FETCH_TIMEOUT_SECS)
        .clamp(1, MAX_FETCH_TIMEOUT_SECS);
    let page = fetch_page_content(url, format, timeout_secs).await?;
    let source = match page.source {
        FetchSource::Reader => "reader",
        FetchSource::Direct => "direct",
    };
    let fallback_note = page
        .reader_error
        .as_deref()
        .map(|error| format!("\nReader-Fallback: {error}"))
        .unwrap_or_default();
    Ok(format!(
        "Fetched URL: {url}\nContent-Type: {}\nExtraction: {source}{fallback_note}\n\n{}",
        if page.content_type.trim().is_empty() {
            "unknown"
        } else {
            page.content_type.trim()
        },
        truncate_chars(page.body.trim(), MAX_FETCH_OUTPUT_CHARS)
    ))
}

/// 按架构化抓取链路获取网页内容，适用于优先拿干净正文再 direct fallback。
async fn fetch_page_content(
    url: &str,
    format: &str,
    timeout_secs: u64,
) -> AppResult<FetchPageContent> {
    if format != "html" {
        // 为什么需要它:
        // 触发条件: 模型请求 markdown/text 网页正文。
        // 不能直接用常规路径的原因: direct HTML 噪声大且更耗 token。
        // 防止副作用: reader 异常时仍能通过 direct 保持工具可用。
        match fetch_via_jina(url, format, timeout_secs).await {
            Ok(page) if page.body.chars().count() >= MIN_READER_CONTENT_CHARS => return Ok(page),
            Ok(page) => {
                let reason = format!(
                    "reader returned too little content ({} chars)",
                    page.body.chars().count()
                );
                return fetch_direct(url, format, timeout_secs, Some(reason)).await;
            }
            Err(error) => {
                return fetch_direct(url, format, timeout_secs, Some(error.to_string())).await;
            }
        }
    }
    fetch_direct(url, format, timeout_secs, None).await
}

/// 通过 reader 服务抓取正文，适用于 markdown/text 这类模型可读内容。
async fn fetch_via_jina(url: &str, format: &str, timeout_secs: u64) -> AppResult<FetchPageContent> {
    let reader_url = format!("{JINA_READER_PREFIX}{url}");
    let mut request = web_http_client()?
        .get(reader_url)
        .header(ACCEPT, "text/plain")
        .header("X-Return-Format", "markdown")
        .header("X-No-Cache", "false");
    if let Some(api_key) = std::env::var("JINA_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        let value = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|error| {
            AppError::Tool(format!("invalid JINA_API_KEY header value: {error}"))
        })?;
        request = request.header(AUTHORIZATION, value);
    }
    let response = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        request.send().await.map_err(AppError::from)
    })
    .await
    .map_err(|_| AppError::Tool("webfetch reader request timed out".to_string()))??;
    if !response.status().is_success() {
        return Err(AppError::Tool(format!(
            "reader status {}",
            response.status()
        )));
    }
    let bytes = read_limited_response(response).await?;
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let body = render_reader_body(&raw, format);
    Ok(FetchPageContent {
        body,
        content_type: "text/markdown".to_string(),
        source: FetchSource::Reader,
        reader_error: None,
    })
}

/// 直接抓取原始 URL，适用于 reader 失败或用户明确要求 html 的场景。
async fn fetch_direct(
    url: &str,
    format: &str,
    timeout_secs: u64,
    reader_error: Option<String>,
) -> AppResult<FetchPageContent> {
    let response = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        web_http_client()?
            .get(url)
            .header(ACCEPT, accept_header(format))
            .send()
            .await
            .map_err(AppError::from)
    })
    .await
    .map_err(|_| AppError::Tool("webfetch request timed out".to_string()))??;
    if !response.status().is_success() {
        return Err(AppError::Tool(format!(
            "webfetch request failed with status {}",
            response.status()
        )));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = read_limited_response(response).await?;
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let body = render_fetch_body(&raw, &content_type, format);
    Ok(FetchPageContent {
        body,
        content_type,
        source: FetchSource::Direct,
        reader_error,
    })
}

/// 返回共享 HTTP client，适用于 web 工具复用连接和 TLS 配置。
fn web_http_client() -> AppResult<&'static Client> {
    if let Some(client) = WEB_HTTP_CLIENT.get() {
        return Ok(client);
    }
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("llm-loop/0.1"));
    let client = Client::builder()
        .default_headers(headers)
        .build()
        .map_err(AppError::from)?;
    let _ = WEB_HTTP_CLIENT.set(client);
    WEB_HTTP_CLIENT
        .get()
        .ok_or_else(|| AppError::Tool("web http client unavailable".to_string()))
}

/// 返回搜索服务 URL，适用于可选环境密钥接入。
fn search_endpoint() -> String {
    let Some(api_key) = std::env::var("EXA_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return SEARCH_ENDPOINT.to_string();
    };
    format!("{SEARCH_ENDPOINT}?exaApiKey={}", url_query_escape(&api_key))
}

/// 转义 URL query value，适用于把可选密钥拼到服务地址。
fn url_query_escape(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect::<Vec<_>>(),
        })
        .collect()
}

/// 校验枚举参数，适用于避免模型传入后端不支持的值。
fn normalized_choice<'a>(
    value: Option<&'a str>,
    allowed: &[&'static str],
    default: &'static str,
    field: &str,
) -> AppResult<&'a str> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    if allowed.contains(&value) {
        Ok(value)
    } else {
        Err(AppError::Tool(format!(
            "{field} must be one of: {}",
            allowed.join(", ")
        )))
    }
}

/// 解析搜索服务响应，适用于 JSON 和 SSE 两种返回格式。
fn parse_search_response(body: &str) -> AppResult<String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{')
        && let Some(text) = parse_search_payload(trimmed)?
    {
        return Ok(text);
    }
    for line in body.lines() {
        let Some(payload) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Some(text) = parse_search_payload(payload)? {
            return Ok(text);
        }
    }
    Err(AppError::Tool(
        "websearch response did not contain search results".to_string(),
    ))
}

/// 解析单个搜索 JSON payload，适用于 MCP content 数组格式。
fn parse_search_payload(payload: &str) -> AppResult<Option<String>> {
    let value: Value = serde_json::from_str(payload)?;
    if value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .is_some()
    {
        return Err(AppError::Tool("websearch request failed".to_string()));
    }
    let Some(content) = value
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    Ok(content
        .iter()
        .find_map(|item| item.get("text").and_then(Value::as_str))
        .map(ToOwned::to_owned))
}

/// 按上限读取响应体，适用于防止 webfetch 拉爆内存。
async fn read_limited_response(response: reqwest::Response) -> AppResult<Vec<u8>> {
    if let Some(length) = response.content_length()
        && length > MAX_FETCH_BYTES as u64
    {
        return Err(AppError::Tool(format!(
            "webfetch response exceeds {} bytes",
            MAX_FETCH_BYTES
        )));
    }
    let mut output = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if output.len().saturating_add(chunk.len()) > MAX_FETCH_BYTES {
            return Err(AppError::Tool(format!(
                "webfetch response exceeds {} bytes",
                MAX_FETCH_BYTES
            )));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

/// 返回 Accept 头，适用于按模型请求格式偏好网页内容。
fn accept_header(format: &str) -> &'static str {
    match format {
        "html" => "text/html,application/xhtml+xml,text/plain;q=0.8,*/*;q=0.1",
        "text" => "text/plain,text/html;q=0.8,*/*;q=0.1",
        _ => "text/markdown,text/plain;q=0.9,text/html;q=0.8,*/*;q=0.1",
    }
}

/// 渲染 reader 正文，适用于去掉 reader 元信息并按格式输出。
fn render_reader_body(raw: &str, format: &str) -> String {
    let mut lines = raw.lines();
    let mut output = Vec::new();
    while let Some(line) = lines.next() {
        if line.starts_with("Title:") {
            continue;
        }
        if line.starts_with("URL Source:") {
            continue;
        }
        output.push(strip_markdown_images(line));
    }
    let markdown = output.join("\n").replace("\n\n\n\n", "\n\n\n");
    if format == "text" {
        normalize_text(&markdown)
    } else {
        markdown.trim().to_string()
    }
}

/// 删除 markdown 图片，适用于降低网页正文中的无用 token。
fn strip_markdown_images(line: &str) -> String {
    let mut output = String::new();
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        output.push_str(&rest[..start]);
        let Some(close_alt) = rest[start + 2..].find("](") else {
            output.push_str(&rest[start..]);
            return output;
        };
        let url_start = start + 2 + close_alt + 2;
        let Some(close_url) = rest[url_start..].find(')') else {
            output.push_str(&rest[start..]);
            return output;
        };
        rest = &rest[url_start + close_url + 1..];
    }
    output.push_str(rest);
    output
}

/// 渲染抓取正文，适用于 HTML 页面转可读文本。
fn render_fetch_body(raw: &str, content_type: &str, format: &str) -> String {
    if format == "html" {
        return raw.to_string();
    }
    if content_type.to_ascii_lowercase().contains("text/html") {
        return extract_html_text(raw);
    }
    raw.to_string()
}

/// 从 HTML 提取可读文本，适用于不引入额外解析依赖的轻量路径。
fn extract_html_text(html: &str) -> String {
    let mut output = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;
    let mut chars = html.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(end_tag) = skip_until {
            if ch == '<' {
                let mut candidate = String::from("<");
                while let Some(next) = chars.peek().copied() {
                    candidate.push(next);
                    chars.next();
                    if next == '>' {
                        break;
                    }
                }
                if candidate.to_ascii_lowercase().starts_with(end_tag) {
                    skip_until = None;
                    output.push(' ');
                }
            }
            continue;
        }
        if in_tag {
            if ch == '>' {
                let lower = tag.trim().to_ascii_lowercase();
                if lower.starts_with("script") {
                    skip_until = Some("</script");
                } else if lower.starts_with("style") {
                    skip_until = Some("</style");
                } else if is_block_tag(&lower) {
                    output.push('\n');
                }
                tag.clear();
                in_tag = false;
            } else {
                tag.push(ch);
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            tag.clear();
        } else {
            output.push(ch);
        }
    }
    normalize_text(&decode_basic_entities(&output))
}

/// 判断 HTML 块级标签，适用于提取文本时保留基本换行。
fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag.trim_start_matches('/').split_whitespace().next(),
        Some("p")
            | Some("br")
            | Some("div")
            | Some("section")
            | Some("article")
            | Some("header")
            | Some("footer")
            | Some("li")
            | Some("ul")
            | Some("ol")
            | Some("h1")
            | Some("h2")
            | Some("h3")
            | Some("h4")
            | Some("h5")
            | Some("h6")
            | Some("tr")
            | Some("table")
    )
}

/// 解码常见 HTML entity，适用于网页正文最小可读化。
fn decode_basic_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// 归一化空白，适用于降低网页正文噪声。
fn normalize_text(text: &str) -> String {
    let mut output = String::new();
    let mut blank_lines = 0_u8;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_lines = blank_lines.saturating_add(1);
            if blank_lines <= 1 {
                output.push('\n');
            }
            continue;
        }
        blank_lines = 0;
        output.push_str(&line);
        output.push('\n');
    }
    output.trim().to_string()
}

/// 按字符截断文本，适用于控制 tool output 大小。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut output = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        output.push_str("\n...[truncated]");
    }
    output
}
