//! Web tools: `fetch_url` reads one page as text, `web_search` asks the
//! person's own SearXNG instance.
//!
//! Both approve per site inside `execute` (see [`super::is_site_tool`]): the
//! action's `command` is the host, so rules and "allow in this chat" grants
//! name a site. Everything they return is untrusted data. A redirect to a
//! different host is not followed: the model gets the target and must call
//! again, so the new site gets its own approval.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use autoagents_derive::{tool, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    async_trait, check_approval_permitted, parse_tool_args, tool_error, ApprovalHook, RiskLevel,
    ToolAction, ToolCallError, ToolRuntime, ToolT, READ_PAGE_CHARS,
};

/// Largest response body read; the rest is dropped and the result says so.
pub const FETCH_MAX_BYTES: usize = 5 * 1024 * 1024;
/// Whole request deadline, including redirects and the body.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Same-host redirects followed before giving up.
pub const MAX_REDIRECTS: usize = 5;
/// Pages kept so `offset` paging does not download again.
const PAGE_CACHE: usize = 8;
/// Searches one run may make.
pub const SEARCHES_PER_RUN: usize = 30;
/// Minimum gap between searches, so upstream engines do not suspend the instance.
pub const SEARCH_INTERVAL: Duration = Duration::from_secs(1);
/// Largest SearXNG response read.
const SEARCH_MAX_BYTES: usize = 2 * 1024 * 1024;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Results returned when the model does not say.
const DEFAULT_RESULTS: usize = 10;
const MAX_RESULTS: usize = 20;

const UNTRUSTED_NOTE: &str = crate::prompts::TOOL_WEB_UNTRUSTED;
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Themis/",
    env!("CARGO_PKG_VERSION")
);

/// The web tools for one run: `fetch_url` always, `web_search` when a SearXNG
/// URL is configured.
pub fn web_tools(
    approvals: Arc<dyn ApprovalHook>,
    config: &crate::configuration::WebConfig,
    permissions: Option<Arc<super::site_permissions::SitePermissions>>,
) -> Vec<Box<dyn ToolT>> {
    let mut fetch = FetchUrlTool::new(Arc::clone(&approvals));
    fetch.permissions = permissions;
    let mut tools: Vec<Box<dyn ToolT>> = vec![Box::new(fetch)];
    if let Some(base) = config
        .search_url
        .as_deref()
        .and_then(|url| reqwest::Url::parse(url).ok())
    {
        tools.push(Box::new(WebSearchTool::new(approvals, base)));
    }
    tools
}

// ---------------------------------------------------------------------------
// Page text
// ---------------------------------------------------------------------------

/// The readable text of one response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageText {
    pub title: String,
    pub text: String,
}

/// Turns a response body into text: HTML becomes the main content as Markdown,
/// text, JSON and XML pass through, anything else is refused.
pub fn page_text(body: &[u8], content_type: &str, url: Option<&str>) -> Result<PageText, String> {
    let mut parts = content_type.split(';');
    let mime = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let charset = parts.find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key.trim().eq_ignore_ascii_case("charset"))
            .then(|| value.trim().trim_matches('"').to_owned())
    });
    let html = matches!(mime.as_str(), "text/html" | "application/xhtml+xml")
        || (mime.is_empty() && looks_like_html(body));
    let textual = html
        || mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
        )
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
        || (mime.is_empty() && std::str::from_utf8(body).is_ok());
    if !textual {
        return Err(format!(
            "{} is not text; fetch_url reads HTML, text, JSON and XML only",
            if mime.is_empty() {
                "this response"
            } else {
                mime.as_str()
            }
        ));
    }
    let charset = charset.or_else(|| html.then(|| meta_charset(body)).flatten());
    let decoded = decode(body, charset.as_deref());
    if html {
        return Ok(html_to_markdown(&decoded, url));
    }
    Ok(PageText {
        title: String::new(),
        text: decoded.trim().to_owned(),
    })
}

fn looks_like_html(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html")
}

/// `charset` from a `<meta>` tag in the first KiB.
fn meta_charset(body: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
    let start = head.find("charset=")? + "charset=".len();
    let value: String = head[start..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    (!value.is_empty()).then_some(value)
}

fn decode(body: &[u8], charset: Option<&str>) -> String {
    let encoding = charset
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(body).0.into_owned()
}

/// The main content of an HTML page as Markdown (Readability), or the whole
/// body without scripts, styles and navigation when no article is found.
pub fn html_to_markdown(html: &str, url: Option<&str>) -> PageText {
    let config = dom_smoothie::Config {
        text_mode: dom_smoothie::TextMode::Markdown,
        max_elements_to_parse: 100_000,
        ..Default::default()
    };
    let url = url.filter(|url| reqwest::Url::parse(url).is_ok());
    let article = dom_smoothie::Readability::new(html, url, Some(config))
        .ok()
        .and_then(|mut readability| readability.parse().ok())
        .filter(|article| !article.text_content.trim().is_empty());
    let (title, text) = match article {
        Some(article) => (article.title, article.text_content.to_string()),
        None => (
            html_title(html),
            htmd::HtmlToMarkdown::builder()
                .skip_tags(vec![
                    "script", "style", "noscript", "svg", "iframe", "head", "nav", "footer",
                    "template", "canvas", "form",
                ])
                .build()
                .convert(html)
                .unwrap_or_default(),
        ),
    };
    PageText {
        title: collapse_space(&title),
        text: collapse_blank_lines(&text),
    }
}

fn html_title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(open) = lower.find("<title") else {
        return String::new();
    };
    let Some(start) = lower[open..].find('>').map(|i| open + i + 1) else {
        return String::new();
    };
    let end = lower[start..]
        .find("</title")
        .map_or(lower.len(), |i| start + i);
    html.get(start..end).unwrap_or_default().trim().to_owned()
}

fn collapse_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drops Markdown's punctuation escapes (`line\.` → `line.`): the text is read
/// by a model, not rendered, and the escapes only add noise.
fn unescape_punctuation(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars
                .peek()
                .copied()
                .filter(|n| ".-_*#+!()[]>|`~=".contains(*n))
            {
                out.push(next);
                chars.next();
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Trims trailing spaces and keeps at most one blank line in a row.
fn collapse_blank_lines(text: &str) -> String {
    let text = unescape_punctuation(text);
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned()
}

/// `(page, next_offset)`: up to [`READ_PAGE_CHARS`] characters from `offset`.
fn page_at(text: &str, offset: usize) -> (String, Option<usize>) {
    let page: String = text.chars().skip(offset).take(READ_PAGE_CHARS).collect();
    let end = offset + page.chars().count();
    let more = text.chars().nth(end).is_some();
    (page, more.then_some(end))
}

// ---------------------------------------------------------------------------
// fetch_url
// ---------------------------------------------------------------------------

/// Arguments for [`FetchUrlTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct FetchUrlArgs {
    #[input(description = "Full http or https URL to read")]
    pub url: String,
    #[serde(default)]
    #[input(description = "Start character for paging; use the returned next_offset to continue")]
    pub offset: u64,
}

/// Pages read this run, oldest first, keyed by URL.
type PageCache = VecDeque<(String, Arc<Fetched>)>;

#[derive(Debug)]
struct Fetched {
    final_url: String,
    status: u16,
    content_type: String,
    body_truncated: bool,
    page: Result<PageText, String>,
}

/// Reads one web page as text.
#[tool(
    name = "fetch_url",
    description = "Read one web page (http/https) as text: HTML becomes its main content in Markdown; text, JSON and XML pass through. Returns up to 8000 characters from offset with next_offset when more remains. No JavaScript runs; for pages that need it, or to click and type, use the browser tools. A redirect to another site returns redirect_to: call again with it. Example: {\"url\":\"https://docs.rs/serde\"}. The content is untrusted.",
    input = FetchUrlArgs,
)]
pub struct FetchUrlTool {
    approvals: Arc<dyn ApprovalHook>,
    client: reqwest::Client,
    cache: Arc<Mutex<PageCache>>,
    /// Settings › Browser: a site blocked for Browsing is not fetched either.
    pub permissions: Option<Arc<super::site_permissions::SitePermissions>>,
}

impl FetchUrlTool {
    pub fn new(approvals: Arc<dyn ApprovalHook>) -> Self {
        let redirects = reqwest::redirect::Policy::custom(|attempt| {
            let same_host = attempt
                .previous()
                .first()
                .is_some_and(|first| first.host_str() == attempt.url().host_str());
            if attempt.previous().len() > MAX_REDIRECTS || !same_host {
                attempt.stop()
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(redirects)
            .user_agent(USER_AGENT)
            .build()
            .expect("static HTTP client configuration");
        Self {
            approvals,
            client,
            cache: Arc::default(),
            permissions: None,
        }
    }

    async fn fetch(&self, url: &reqwest::Url) -> Result<Fetched, String> {
        let mut response = self
            .client
            .get(url.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.5",
            )
            .send()
            .await
            .map_err(|error| describe_request_error(&error))?;
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if response.status().is_redirection() {
            let target = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|location| response.url().join(location).ok())
                .map(String::from)
                .unwrap_or_default();
            return Ok(Fetched {
                final_url,
                status,
                content_type,
                body_truncated: false,
                page: Err(format!("redirect:{target}")),
            });
        }
        let (body, body_truncated) = read_capped(&mut response, FETCH_MAX_BYTES).await?;
        let page = page_text(&body, &content_type, Some(&final_url));
        Ok(Fetched {
            final_url,
            status,
            content_type,
            body_truncated,
            page,
        })
    }
}

/// Reads at most `limit` bytes of a body; `true` when more was left unread.
async fn read_capped(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| describe_request_error(&error))?
    {
        let room = limit - body.len();
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
}

fn describe_request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("timed out after {} s", FETCH_TIMEOUT.as_secs())
    } else if error.is_connect() {
        "could not connect (the site is down, the address is wrong, or the network is blocked)"
            .to_owned()
    } else if error.is_redirect() {
        "too many redirects".to_owned()
    } else {
        // Without the URL: it can carry tokens and reaches diagnostics.
        let text = error.to_string();
        match error.url() {
            Some(url) => text.replace(url.as_str(), "<url>"),
            None => text,
        }
    }
}

/// A URL the web tools accept: http(s), a host, no credentials.
pub fn checked_url(text: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(text.trim()).map_err(|_| {
        format!(
            "'{}' is not a full URL; include https://",
            truncate(text, 200)
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "only http and https URLs are supported, not {}:",
            url.scheme()
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with a user name or password are not supported".to_owned());
    }
    Ok(url)
}

/// The site an approval names: the URL's host, lowercased.
pub fn site_of(url: &reqwest::Url) -> String {
    url.host_str().unwrap_or_default().to_ascii_lowercase()
}

fn truncate(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// Approval for one web action on `site`.
pub(crate) fn approve_site(
    approvals: &dyn ApprovalHook,
    tool: &str,
    site: &str,
    summary: String,
    risk: RiskLevel,
) -> Result<(), ToolCallError> {
    check_approval_permitted(
        approvals,
        &ToolAction {
            display: None,
            tool: tool.to_owned(),
            summary,
            risk,
            command: vec![site.to_owned()],
        },
    )
}

#[async_trait]
impl ToolRuntime for FetchUrlTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: FetchUrlArgs = parse_tool_args("fetch_url", args)?;
        let url = checked_url(&args.url).map_err(|e| tool_error(format!("fetch_url: {e}")))?;
        let key = url.to_string();
        let cached = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(cached, _)| *cached == key)
            .map(|(_, page)| Arc::clone(page));
        let fetched = match cached {
            // Paging through a page already read (and approved) this run.
            Some(page) => page,
            None => {
                let site = site_of(&url);
                if self.permissions.as_ref().is_some_and(|permissions| {
                    permissions.decide(super::site_permissions::Permission::Browsing, &site)
                        == crate::configuration::SiteAccess::Block
                }) {
                    return Err(tool_error(format!(
                        "fetch_url: {site} is blocked in Settings › Browser. {}",
                        super::REFUSAL_NOTE
                    )));
                }
                approve_site(
                    self.approvals.as_ref(),
                    "fetch_url",
                    &site_of(&url),
                    format!("fetch_url {}", truncate(&key, 300)),
                    RiskLevel::Network,
                )?;
                let fetched = Arc::new(
                    self.fetch(&url)
                        .await
                        .map_err(|e| tool_error(format!("fetch_url: {e}")))?,
                );
                let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() >= PAGE_CACHE {
                    cache.pop_front();
                }
                cache.push_back((key.clone(), Arc::clone(&fetched)));
                fetched
            }
        };
        let page = match &fetched.page {
            Ok(page) => page,
            Err(message) => {
                if let Some(target) = message.strip_prefix("redirect:") {
                    return Ok(json!({
                        "url": key,
                        "status": fetched.status,
                        "redirect_to": target,
                        "note": crate::prompts::TOOL_WEB_REDIRECT,
                    }));
                }
                return Err(tool_error(format!("fetch_url: {message}")));
            }
        };
        let offset = usize::try_from(args.offset).unwrap_or(usize::MAX);
        let (content, next_offset) = page_at(&page.text, offset);
        let mut result = json!({
            "url": key,
            "final_url": fetched.final_url,
            "status": fetched.status,
            "content_type": fetched.content_type,
            "title": page.title,
            "offset": offset,
            "content": content,
            "total_chars": page.text.chars().count(),
            "untrusted": true,
            "note": UNTRUSTED_NOTE,
        });
        if let Some(next) = next_offset {
            result["next_offset"] = json!(next);
        }
        if fetched.body_truncated {
            result["body_truncated"] = json!(format!(
                "only the first {} MiB of the response was read",
                FETCH_MAX_BYTES / (1024 * 1024)
            ));
        }
        if !(200..300).contains(&fetched.status) {
            result["warning"] = json!(format!(
                "HTTP {}: this is an error page, not the content you asked for",
                fetched.status
            ));
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// web_search
// ---------------------------------------------------------------------------

/// Arguments for [`WebSearchTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToolInput)]
pub struct WebSearchArgs {
    #[input(description = "Search words, as you would type them into a search engine")]
    pub query: String,
    #[serde(default)]
    #[input(
        description = "Only results from the last day, week, month or year; omit for any time"
    )]
    pub time_range: String,
    #[serde(default)]
    #[input(description = "Results to return (default 10, max 20)")]
    pub max_results: u64,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    pub engines: Vec<String>,
}

/// One engine that did not answer, with SearXNG's reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedEngine {
    pub engine: String,
    pub reason: String,
}

/// A SearXNG JSON response reduced to what the model needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SearchResults {
    pub results: Vec<SearchHit>,
    pub failed_engines: Vec<FailedEngine>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
}

/// Parses a SearXNG `format=json` body. The body is untrusted: fields of the
/// wrong type are skipped, text is capped and only http(s) result URLs are kept.
/// The result count is the results array (`number_of_results` is unreliable).
pub fn parse_searxng(body: &[u8], max_results: usize) -> Result<SearchResults, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "SearXNG did not return JSON".to_owned())?;
    let text = |value: &Value, limit: usize| {
        value
            .as_str()
            .map(|text| truncate(&collapse_space(text), limit))
            .unwrap_or_default()
    };
    let mut seen = std::collections::HashSet::new();
    let results = value["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let url = item["url"].as_str()?;
            let parsed = checked_url(url).ok()?;
            if url.len() > 2048 || !seen.insert(parsed.to_string()) {
                return None;
            }
            let engines = match &item["engines"] {
                Value::Array(list) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .take(8)
                    .map(|e| truncate(e, 40))
                    .collect(),
                _ => item["engine"]
                    .as_str()
                    .map(|e| vec![truncate(e, 40)])
                    .unwrap_or_default(),
            };
            Some(SearchHit {
                title: text(&item["title"], 300),
                url: parsed.to_string(),
                snippet: text(&item["content"], 500),
                published: item["publishedDate"]
                    .as_str()
                    .filter(|date| !date.is_empty())
                    .map(|date| truncate(date, 40)),
                engines,
            })
        })
        .take(max_results)
        .collect();
    let failed_engines = value["unresponsive_engines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let pair = entry.as_array()?;
            Some(FailedEngine {
                engine: truncate(pair.first()?.as_str()?, 40),
                reason: pair
                    .get(1)
                    .and_then(Value::as_str)
                    .map(|reason| truncate(reason, 120))
                    .unwrap_or_else(|| "no answer".to_owned()),
            })
        })
        .take(32)
        .collect();
    let answers = value["answers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|answer| match answer {
            Value::String(text) => Some(truncate(&collapse_space(text), 300)),
            Value::Object(fields) => fields
                .get("answer")
                .and_then(Value::as_str)
                .map(|text| truncate(&collapse_space(text), 300)),
            _ => None,
        })
        .take(3)
        .collect();
    Ok(SearchResults {
        results,
        failed_engines,
        answers,
    })
}

struct Pacing {
    searches: usize,
    last: Option<Instant>,
}

/// Searches the web through the person's SearXNG instance.
#[tool(
    name = "web_search",
    description = "Search the web. Returns titles, URLs and snippets (untrusted), plus failed_engines when some search engines did not answer, so few results may mean an outage rather than nothing to find. Read a result with fetch_url. Example: {\"query\":\"tauri 2 multiwebview\",\"time_range\":\"month\"}.",
    input = WebSearchArgs,
)]
pub struct WebSearchTool {
    approvals: Arc<dyn ApprovalHook>,
    client: reqwest::Client,
    base: reqwest::Url,
    pacing: Arc<tokio::sync::Mutex<Pacing>>,
}

impl WebSearchTool {
    pub fn new(approvals: Arc<dyn ApprovalHook>, base: reqwest::Url) -> Self {
        let client = reqwest::Client::builder()
            .timeout(SEARCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .expect("static HTTP client configuration");
        Self {
            approvals,
            client,
            base,
            pacing: Arc::new(tokio::sync::Mutex::new(Pacing {
                searches: 0,
                last: None,
            })),
        }
    }

    fn search_url(&self, args: &WebSearchArgs) -> Result<reqwest::Url, String> {
        let mut url = self.base.clone();
        let path = format!("{}/search", url.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(None);
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("q", args.query.trim())
                .append_pair("format", "json")
                .append_pair("categories", "general");
            match args.time_range.trim().to_ascii_lowercase().as_str() {
                "" | "any" => {}
                range @ ("day" | "week" | "month" | "year") => {
                    query.append_pair("time_range", range);
                }
                other => {
                    return Err(format!(
                        "time_range must be day, week, month or year, not '{}'",
                        truncate(other, 20)
                    ))
                }
            }
        }
        Ok(url)
    }
}

#[async_trait]
impl ToolRuntime for WebSearchTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let args: WebSearchArgs = parse_tool_args("web_search", args)?;
        let query = args.query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err(tool_error(
                "web_search: query must be 1..500 characters".to_owned(),
            ));
        }
        let url = self
            .search_url(&args)
            .map_err(|e| tool_error(format!("web_search: {e}")))?;
        approve_site(
            self.approvals.as_ref(),
            "web_search",
            &site_of(&self.base),
            format!("web_search {}", truncate(query, 200)),
            RiskLevel::Network,
        )?;
        {
            let mut pacing = self.pacing.lock().await;
            if pacing.searches >= SEARCHES_PER_RUN {
                return Err(tool_error(format!(
                    "web_search: this run already made {SEARCHES_PER_RUN} searches; work with the results you have"
                )));
            }
            if let Some(wait) = pacing
                .last
                .and_then(|last| SEARCH_INTERVAL.checked_sub(last.elapsed()))
            {
                tokio::time::sleep(wait).await;
            }
            pacing.searches += 1;
            pacing.last = Some(Instant::now());
        }
        let base = self.base.as_str().trim_end_matches('/');
        let mut response = self.client.get(url).send().await.map_err(|error| {
            tool_error(if error.is_connect() {
                format!("web_search: SearXNG is not reachable at {base}. Start it, or fix web.search_url in runtime.jsonc")
            } else {
                format!("web_search: {}", describe_request_error(&error))
            })
        })?;
        let status = response.status().as_u16();
        match status {
            200..=299 => {}
            403 => return Err(tool_error(format!(
                "web_search: SearXNG at {base} refused JSON output (HTTP 403). Add json to search.formats in its settings.yml and restart it"
            ))),
            429 => return Err(tool_error(
                "web_search: SearXNG is rate limiting (HTTP 429); wait before searching again".to_owned(),
            )),
            _ => return Err(tool_error(format!("web_search: SearXNG returned HTTP {status}"))),
        }
        let (body, _) = read_capped(&mut response, SEARCH_MAX_BYTES)
            .await
            .map_err(|e| tool_error(format!("web_search: {e}")))?;
        let limit = match usize::try_from(args.max_results).unwrap_or(MAX_RESULTS) {
            0 => DEFAULT_RESULTS,
            n => n.min(MAX_RESULTS),
        };
        let parsed =
            parse_searxng(&body, limit).map_err(|e| tool_error(format!("web_search: {e}")))?;
        let mut result = serde_json::to_value(&parsed).unwrap_or_default();
        result["query"] = json!(query);
        result["untrusted"] = json!(true);
        if parsed.results.is_empty() && !parsed.failed_engines.is_empty() {
            result["note"] = json!(crate::prompts::TOOL_WEB_NO_RESULTS);
        } else {
            result["note"] = json!(UNTRUSTED_NOTE);
        }
        Ok(result)
    }
}

/// Settings › Web: asks the SearXNG instance at `base` one test search and says how it
/// answered: `working` (results, perhaps with some engines failing), `degraded` (no
/// results), `json_off` (HTTP 403),
/// `rate_limited`, `unreachable`, `invalid` (not an http(s) URL) or `error`.
pub async fn check_search_server(base: &str) -> Value {
    let base_url = match checked_url(base.trim()) {
        Ok(url) => url,
        Err(error) => return json!({"state": "invalid", "message": error}),
    };
    let mut url = base_url.clone();
    let path = format!("{}/search", url.path().trim_end_matches('/'));
    url.set_path(&path);
    url.set_query(Some("q=weather&format=json&categories=general"));
    let client = match reqwest::Client::builder()
        .timeout(SEARCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(client) => client,
        Err(error) => return json!({"state": "error", "message": error.to_string()}),
    };
    let mut response = match client.get(url).send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() || error.is_timeout() => {
            return json!({"state": "unreachable", "message": format!("Not reachable at {}", base.trim())})
        }
        Err(error) => return json!({"state": "error", "message": describe_request_error(&error)}),
    };
    match response.status().as_u16() {
        200..=299 => {}
        403 => {
            return json!({"state": "json_off", "message": "JSON output is off: add json to search.formats in its settings.yml"})
        }
        429 => return json!({"state": "rate_limited", "message": "Rate limiting (HTTP 429)"}),
        status => return json!({"state": "error", "message": format!("HTTP {status}")}),
    }
    let parsed = match read_capped(&mut response, SEARCH_MAX_BYTES)
        .await
        .and_then(|(body, _)| parse_searxng(&body, MAX_RESULTS))
    {
        Ok(parsed) => parsed,
        Err(error) => {
            return json!({"state": "error", "message": format!("Not a SearXNG answer: {error}")})
        }
    };
    // Some engine is nearly always busy or asking for a CAPTCHA; results still mean it works.
    let state = if parsed.results.is_empty() {
        "degraded"
    } else {
        "working"
    };
    json!({"state": state, "results": parsed.results.len(), "failed_engines": parsed.failed_engines})
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn html_article_becomes_markdown_with_title() {
        let html = r#"<!doctype html><html><head><title>Ignored &amp; tab</title></head><body>
            <nav><a href="/">Home</a> <a href="/x">Menu item</a></nav>
            <article><h1>Serde guide</h1>
            <p>Serde is a framework for <strong>serializing</strong> and deserializing Rust data structures efficiently and generically. It is used everywhere.</p>
            <p>Derive macros generate the code for you, so most structs need one attribute only and nothing else at all.</p>
            <ul><li>Fast</li><li>Generic</li></ul></article>
            <script>alert("x")</script></body></html>"#;
        let page = page_text(
            html.as_bytes(),
            "text/html; charset=utf-8",
            Some("https://serde.rs/"),
        )
        .unwrap();
        assert!(page.text.contains("**serializing**"), "{}", page.text);
        assert!(page.text.contains("Derive macros"));
        assert!(!page.text.contains("alert("));
        assert!(!page.title.is_empty());
    }

    #[test]
    fn page_without_article_falls_back_to_body_text() {
        let html = "<html><head><title> Short </title><style>p{}</style></head><body><p>Just one line.</p><script>x()</script></body></html>";
        let page = html_to_markdown(html, None);
        assert_eq!(page.title, "Short");
        assert!(page.text.contains("Just one line."), "{page:?}");
        assert!(!page.text.contains("x()"));
    }

    #[test]
    fn text_json_and_charsets_pass_through_and_binary_is_refused() {
        assert_eq!(
            page_text(b"{\"a\":1}", "application/json", None)
                .unwrap()
                .text,
            "{\"a\":1}"
        );
        assert_eq!(
            page_text(b"plain", "text/plain", None).unwrap().text,
            "plain"
        );
        // ISO-8859-1 "café" decoded from its label.
        assert_eq!(
            page_text(b"caf\xe9", "text/plain; charset=iso-8859-1", None)
                .unwrap()
                .text,
            "café"
        );
        let error = page_text(b"%PDF-1.7", "application/pdf", None).unwrap_err();
        assert!(error.contains("application/pdf"), "{error}");
        assert!(page_text(&[0xff, 0x00, 0xfe], "", None).is_err());
    }

    #[test]
    fn paging_returns_next_offset_until_the_end() {
        let text = "x".repeat(READ_PAGE_CHARS + 5);
        let (first, next) = page_at(&text, 0);
        assert_eq!(first.chars().count(), READ_PAGE_CHARS);
        assert_eq!(next, Some(READ_PAGE_CHARS));
        let (rest, next) = page_at(&text, READ_PAGE_CHARS);
        assert_eq!(rest.len(), 5);
        assert_eq!(next, None);
        assert_eq!(page_at(&text, usize::MAX).0, "");
    }

    #[test]
    fn urls_need_http_a_host_and_no_credentials() {
        assert_eq!(
            site_of(&checked_url(" https://Docs.RS/serde ").unwrap()),
            "docs.rs"
        );
        for bad in [
            "docs.rs",
            "file:///etc/passwd",
            "ftp://x.org",
            "https://user:pw@x.org",
            "javascript:alert(1)",
        ] {
            assert!(checked_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn searxng_results_failed_engines_and_answers_are_kept() {
        let body = br#"{"query":"q","number_of_results":0,
            "results":[
              {"url":"https://a.example/x","title":"A  title","content":"snippet","engines":["brave","google cse"],"publishedDate":"2026-10-20T10:00:00"},
              {"url":"https://a.example/x","title":"duplicate"},
              {"url":"javascript:alert(1)","title":"bad"},
              {"url":"http://b.example","title":5,"engine":"duckduckgo"}],
            "answers":["42",{"answer":"forty-two"},7],
            "unresponsive_engines":[["google","Suspended: access denied"],["bing"],[3]]}"#;
        let parsed = parse_searxng(body, 10).unwrap();
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(parsed.results[0].title, "A title");
        assert_eq!(parsed.results[0].engines, ["brave", "google cse"]);
        assert_eq!(parsed.results[1].title, "");
        assert_eq!(parsed.results[1].engines, ["duckduckgo"]);
        assert_eq!(parsed.answers, ["42", "forty-two"]);
        assert_eq!(
            parsed.failed_engines,
            [
                FailedEngine {
                    engine: "google".into(),
                    reason: "Suspended: access denied".into()
                },
                FailedEngine {
                    engine: "bing".into(),
                    reason: "no answer".into()
                },
            ]
        );
        assert_eq!(parse_searxng(body, 1).unwrap().results.len(), 1);
        assert!(parse_searxng(b"<html>", 10).is_err());
    }

    #[test]
    fn search_url_keeps_a_base_path_and_checks_time_range() {
        let tool = WebSearchTool::new(
            Arc::new(super::super::AllowAllHook),
            reqwest::Url::parse("http://127.0.0.1:8888/searx/").unwrap(),
        );
        let url = tool
            .search_url(&WebSearchArgs {
                query: " rust & tokio ".into(),
                time_range: "Week".into(),
                max_results: 0,
            })
            .unwrap();
        assert_eq!(url.path(), "/searx/search");
        assert_eq!(
            url.query(),
            Some("q=rust+%26+tokio&format=json&categories=general&time_range=week")
        );
        assert!(tool
            .search_url(&WebSearchArgs {
                query: "x".into(),
                time_range: "decade".into(),
                max_results: 0
            })
            .is_err());
    }

    /// A hook that records every action and answers with `decision`.
    fn recording_hook(
        decision: super::super::Approval,
    ) -> (Arc<dyn ApprovalHook>, Arc<Mutex<Vec<ToolAction>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let hook = super::super::FnHook::new(move |action: &ToolAction| {
            log.lock().unwrap().push(action.clone());
            decision
        });
        (Arc::new(hook), seen)
    }

    #[tokio::test]
    async fn fetch_url_approves_the_site_reads_once_and_pages_from_cache() {
        use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let long = format!(
            "<html><body><article><h1>Doc</h1><p>{}</p></article></body></html>",
            "word ".repeat(3000)
        );
        Mock::given(path("/doc"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(long, "text/html"))
            .expect(1)
            .mount(&server)
            .await;
        let (hook, seen) = recording_hook(super::super::Approval::AllowOnce);
        let tool = FetchUrlTool::new(hook);
        let url = format!("{}/doc", server.uri());
        let first = tool.execute(json!({"url": url})).await.unwrap();
        assert_eq!(first["status"], 200);
        assert_eq!(first["untrusted"], true);
        let next = first["next_offset"].as_u64().unwrap();
        let second = tool
            .execute(json!({"url": url, "offset": next}))
            .await
            .unwrap();
        assert_eq!(second["offset"], next);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "paging reuses the approved page");
        assert_eq!(seen[0].tool, "fetch_url");
        assert_eq!(seen[0].command, ["127.0.0.1"]);
        assert_eq!(seen[0].risk, RiskLevel::Network);
        assert_eq!(seen[0].grant_key(), "fetch_url\x00127.0.0.1");
    }

    #[tokio::test]
    async fn fetch_url_stops_at_a_redirect_to_another_site_and_denials_fetch_nothing() {
        use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/same"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/final"))
            .mount(&server)
            .await;
        Mock::given(path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("arrived", "text/plain"))
            .mount(&server)
            .await;
        Mock::given(path("/away"))
            .respond_with(
                ResponseTemplate::new(301).insert_header("location", "https://elsewhere.example/x"),
            )
            .mount(&server)
            .await;
        Mock::given(path("/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_raw("not here", "text/plain"))
            .mount(&server)
            .await;
        let (hook, _) = recording_hook(super::super::Approval::AllowOnce);
        let tool = FetchUrlTool::new(hook);
        let same = tool
            .execute(json!({"url": format!("{}/same", server.uri())}))
            .await
            .unwrap();
        assert_eq!(same["content"], "arrived");
        assert!(same["final_url"].as_str().unwrap().ends_with("/final"));
        let away = tool
            .execute(json!({"url": format!("{}/away", server.uri())}))
            .await
            .unwrap();
        assert_eq!(away["redirect_to"], "https://elsewhere.example/x");
        assert!(away.get("content").is_none());
        let missing = tool
            .execute(json!({"url": format!("{}/missing", server.uri())}))
            .await
            .unwrap();
        assert_eq!(missing["status"], 404);
        assert!(missing["warning"].as_str().unwrap().contains("404"));

        let (deny, _) = recording_hook(super::super::Approval::Deny);
        let denied = FetchUrlTool::new(deny)
            .execute(json!({"url": format!("{}/final", server.uri())}))
            .await
            .unwrap_err();
        assert!(denied.to_string().contains("approval"), "{denied}");
    }

    #[tokio::test]
    async fn the_search_server_check_names_each_way_it_can_fail() {
        use wiremock::{
            matchers::{path, query_param},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(path("/ok/search"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{"title": "Weather", "url": "https://example.com/w", "content": "Sunny"}],
                "unresponsive_engines": []
            })))
            .mount(&server)
            .await;
        Mock::given(path("/some/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [], "unresponsive_engines": [["google", "CAPTCHA"]]
            })))
            .mount(&server)
            .await;
        Mock::given(path("/off/search"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let ok = check_search_server(&format!("{}/ok", server.uri())).await;
        assert_eq!(
            (ok["state"].as_str(), ok["results"].as_u64()),
            (Some("working"), Some(1)),
            "{ok}"
        );
        let some = check_search_server(&format!("{}/some", server.uri())).await;
        assert_eq!(some["state"], "degraded");
        assert_eq!(some["failed_engines"][0]["reason"], "CAPTCHA");
        assert_eq!(
            check_search_server(&format!("{}/off", server.uri())).await["state"],
            "json_off"
        );
        assert_eq!(
            check_search_server("http://127.0.0.1:9").await["state"],
            "unreachable"
        );
        assert_eq!(
            check_search_server("ftp://example.com").await["state"],
            "invalid"
        );
    }

    #[tokio::test]
    async fn web_search_reports_failed_engines_and_explains_http_errors() {
        use wiremock::{
            matchers::{path, query_param},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        Mock::given(path("/search"))
            .and(query_param("q", "ok"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{"url": "https://docs.rs/tokio", "title": "tokio", "content": "runtime", "engines": ["brave"]}],
                "unresponsive_engines": [["google", "Suspended: access denied"]]
            })))
            .mount(&server)
            .await;
        Mock::given(path("/search"))
            .and(query_param("q", "outage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [], "unresponsive_engines": [["brave", "too many requests"]]
            })))
            .mount(&server)
            .await;
        Mock::given(path("/search"))
            .and(query_param("q", "forbidden"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let (hook, seen) = recording_hook(super::super::Approval::AllowOnce);
        let tool = WebSearchTool::new(hook, reqwest::Url::parse(&server.uri()).unwrap());
        let ok = tool.execute(json!({"query": "ok"})).await.unwrap();
        assert_eq!(ok["results"][0]["url"], "https://docs.rs/tokio");
        assert_eq!(ok["failed_engines"][0]["engine"], "google");
        let started = Instant::now();
        let outage = tool.execute(json!({"query": "outage"})).await.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "searches are paced"
        );
        assert!(outage["note"].as_str().unwrap().contains("outage"));
        let forbidden = tool
            .execute(json!({"query": "forbidden"}))
            .await
            .unwrap_err();
        assert!(
            forbidden.to_string().contains("search.formats"),
            "{forbidden}"
        );
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .all(|action| action.command == ["127.0.0.1"]));

        let down = WebSearchTool::new(
            recording_hook(super::super::Approval::AllowOnce).0,
            reqwest::Url::parse("http://127.0.0.1:9/").unwrap(),
        );
        let error = down.execute(json!({"query": "x"})).await.unwrap_err();
        assert!(error.to_string().contains("not reachable"), "{error}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        // Untrusted HTML never panics, and the result is bounded by the input.
        #[test]
        fn any_html_extracts_without_panicking(html in "(<[a-z]{1,6}( [a-z]{1,4}=\"[^\"]{0,8}\")?>|</[a-z]{1,6}>|[^<]{0,40}){0,60}") {
            let page = html_to_markdown(&html, Some("https://example.com/"));
            prop_assert!(page.text.len() <= html.len() * 4 + 64);
        }

        // Untrusted JSON never panics; kept results are http(s), unique and capped.
        #[test]
        fn any_searxng_json_parses_safely(body in proptest::collection::vec(any::<u8>(), 0..400), max in 0usize..25) {
            if let Ok(parsed) = parse_searxng(&body, max) {
                prop_assert!(parsed.results.len() <= max);
            }
        }

        #[test]
        fn searxng_shaped_values_parse_safely(urls in proptest::collection::vec("[a-z]{0,6}:(//)?[a-z.]{0,10}/?[a-z]{0,5}", 0..12), max in 1usize..25) {
            let results: Vec<Value> = urls.iter().map(|url| json!({"url": url, "title": url, "content": url, "engines": [url]})).collect();
            let body = serde_json::to_vec(&json!({"results": results, "unresponsive_engines": [[urls.first(), urls.last()]]})).unwrap();
            let parsed = parse_searxng(&body, max).unwrap();
            prop_assert!(parsed.results.len() <= max);
            let unique: std::collections::HashSet<_> = parsed.results.iter().map(|hit| &hit.url).collect();
            prop_assert_eq!(unique.len(), parsed.results.len());
            for hit in &parsed.results {
                prop_assert!(hit.url.starts_with("http://") || hit.url.starts_with("https://"));
            }
        }
    }
}
