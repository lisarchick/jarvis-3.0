//! Поиск в интернете: DuckDuckGo (html-lite) → параллельная загрузка топ-страниц →
//! очистка до чистого Markdown (без рекламы/навигации) → ранжирование по качеству
//! источника и свежести → кэш с TTL. Контент трактуется как данные, не инструкции.

use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const CACHE_TTL: Duration = Duration::from_secs(600);
const MAX_CACHE: usize = 100;
const PAGE_TIMEOUT: Duration = Duration::from_secs(8);
const FETCH_PAGES: usize = 4;
const PAGE_CHARS: usize = 2000;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Очищенный Markdown со страницы (если удалось загрузить).
    pub content: String,
    pub host: String,
    pub score: f32,
    pub fresh: bool,
}

// ------------------------------------------------------------------ кэш

#[derive(Clone)]
struct CacheEntry {
    results: Vec<SearchResult>,
    ts: Instant,
}

static CACHE: Lazy<Mutex<HashMap<String, CacheEntry>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn cache_get(query: &str) -> Option<Vec<SearchResult>> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(entry) = cache.get(query) {
        if entry.ts.elapsed() < CACHE_TTL {
            return Some(entry.results.clone());
        }
        cache.remove(query);
    }
    None
}

fn cache_put(query: String, results: Vec<SearchResult>) {
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= MAX_CACHE {
        if let Some(oldest) = cache.iter().min_by_key(|(_k, v)| v.ts).map(|(k, _)| k.clone()) {
            cache.remove(&oldest);
        }
    }
    cache.insert(query, CacheEntry { results, ts: Instant::now() });
}

pub fn cache_clear() {
    CACHE.lock().unwrap().clear();
}

// ------------------------------------------------------------------ DDG-серп

/// Поиск через html.duckduckgo.com (без ключей). Возвращает сниппеты.
async fn ddg_search(client: &Client, query: &str) -> Result<Vec<(String, String, String)>, String> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(query));
    let resp = client
        .get(&url)
        .header("User-Agent", UA)
        .header("Accept-Language", "ru-RU,ru;q=0.9,en;q=0.8")
        .send()
        .await
        .map_err(|e| format!("поиск недоступен: {}", e))?;
    let html = resp.text().await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    // Ссылки результатов: <a rel="nofollow" class="result__a" href="...">Title</a>
    static A_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());
    static SNIP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).unwrap());
    let links: Vec<(String, String)> = A_RE
        .captures_iter(&html)
        .map(|c| {
            let href = decode_ddg_url(&c[1]);
            (href, strip_tags(&c[2]))
        })
        .collect();
    let snippets: Vec<String> = SNIP_RE.captures_iter(&html).map(|c| strip_tags(&c[1])).collect();
    for (i, (href, title)) in links.into_iter().enumerate() {
        if href.starts_with("http") && !title.trim().is_empty() {
            let snippet = snippets.get(i).cloned().unwrap_or_default();
            out.push((title, href, snippet));
        }
        if out.len() >= 8 {
            break;
        }
    }
    if out.is_empty() {
        return Err("ничего не найдено".into());
    }
    Ok(out)
}

fn decode_ddg_url(href: &str) -> String {
    // DDG отдаёт редиректы //duckduckgo.com/l/?uddg=<encoded>
    if let Some(pos) = href.find("uddg=") {
        let raw = &href[pos + 5..];
        let cut = raw.split('&').next().unwrap_or(raw);
        return percent_encoding::percent_decode_str(cut).decode_utf8_lossy().into_owned();
    }
    href.to_string()
}

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

// ------------------------------------------------------------------ загрузка страниц

async fn fetch_page(client: &Client, url: &str) -> Option<String> {
    let resp = client.get(url).header("User-Agent", UA).timeout(PAGE_TIMEOUT).send().await.ok()?;
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    if !ct.contains("text/html") {
        return None;
    }
    let html = resp.text().await.ok()?;
    Some(markdown_from_html(&html))
}

// ------------------------------------------------------------------ HTML → чистый Markdown

/// Убирает скрипты/стили/навигацию/рекламу, извлекает основной текст с заголовками.
pub fn markdown_from_html(html: &str) -> String {
    let mut h: String = html.to_string();
    // Вырезаем шум.
    static NOISE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?is)<(script|style|noscript|svg|iframe|form|nav|header|footer|aside)[^>]*>.*?</\1>|<!--.*?-->|<button[^>]*>.*?</button>").unwrap()
    });
    h = NOISE.replace_all(&h, " ").into_owned();

    // Основной контент: самый крупный <article>, <main> или <div id/class="content|article|post">
    let main = extract_main(&h).unwrap_or_else(|| h.clone());

    // Блочные теги → переводы строк.
    static BLOCK: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)</?(p|div|br|li|tr|h[1-6]|section|article|blockquote|pre|table|ul|ol)[^>]*>").unwrap()
    });
    let text = BLOCK.replace_all(&main, "\n").into_owned();

    // Заголовки и списки в Markdown.
    let mut md = String::new();
    static H_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<h([1-6])[^>]*>(.*?)</h\1>").unwrap());
    let text = H_RE
        .replace_all(&text, |caps: &regex::Captures| {
            let level = caps[1].parse::<usize>().unwrap_or(2);
            format!("\n{} {}\n", "#".repeat(level.min(4)), strip_tags(&caps[2]))
        })
        .into_owned();
    static LI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<li[^>]*>(.*?)</li>").unwrap());
    let text = LI_RE
        .replace_all(&text, |caps: &regex::Captures| format!("\n- {}", strip_tags(&caps[1])))
        .into_owned();

    // Оставшиеся теги — снести.
    md.push_str(&strip_tags(&text));

    // Чистка строк: реклама/cookie/подписка/меню-обрывки.
    md = md
        .lines()
        .map(|l| l.trim())
        .filter(|l| {
            let low = l.to_lowercase();
            !(low.is_empty()
                || low.starts_with("cookie")
                || low.contains("подпишитесь на")
                || low.contains("поделиться:")
                || low.starts_with("реклама")
                || low.starts_with("advertis")
                || low.starts_with('·')
                || low.starts_with("| ")
                || low.chars().count() < 2 && !low.starts_with('#')
                || low == "—")
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Схлопнуть пустые повторы и обрезать длину.
    static MULTI_NL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").unwrap());
    md = MULTI_NL.replace_all(&md, "\n\n").into_owned();
    if md.chars().count() > PAGE_CHARS {
        md = md.chars().take(PAGE_CHARS).collect();
    }
    md.trim().to_string()
}

fn extract_main(html: &str) -> Option<String> {
    // <article> или <main> — целиком (от первого открывающего до последнего закрывающего).
    let low = html.to_lowercase();
    for tag in ["article", "main"] {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        if let (Some(s), Some(e)) = (low.find(&open), low.rfind(&close)) {
            if e > s {
                return Some(html[s..e].to_string());
            }
        }
    }
    // div с content/article/post в id/class
    static DIV_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?is)<div[^>]*(?:id|class)="[^"]*(?:content|article|post|entry)[^"]*"[^>]*>(.*?)(?:<footer|</body)"#).unwrap());
    DIV_RE.captures(html).map(|c| c[1].to_string())
}

pub fn strip_tags(html: &str) -> String {
    static TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
    static ENT: Lazy<Vec<(String, String)>> = Lazy::new(|| {
        vec![
            ("&nbsp;".into(), " ".into()),
            ("&amp;".into(), "&".into()),
            ("&lt;".into(), "<".into()),
            ("&gt;".into(), ">".into()),
            ("&quot;".into(), "\"".into()),
            ("&#39;".into(), "'".into()),
            ("&mdash;".into(), "—".into()),
            ("&laquo;".into(), "«".into()),
            ("&raquo;".into(), "»".into()),
        ]
    });
    let mut s = TAGS.replace_all(html, " ").into_owned();
    for (from, to) in ENT.iter() {
        s = s.replace(from, to);
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------ ранжирование

/// Домены с хорошей репутацией для фактов.
const TRUSTED_HOSTS: &[&str] = &[
    "wikipedia.org", "wikimedia.org", "britannica.com", "nature.com", "science.org",
    "nasa.gov", "who.int", "cbr.ru", "gov.ru", "ria.ru", "tass.ru", "lenta.ru",
    "habr.com", "github.com", "stackoverflow.com", "microsoft.com", "openai.com",
    "anthropic.com", "google.com", "docs.python.org", "developer.mozilla.org",
    "rbc.ru", "vedomosti.ru", "kommersant.ru", "medscape.com", "mayoclinic.org",
];

fn host_of(url: &str) -> String {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    rest.split('/').next().unwrap_or("").to_lowercase()
}

fn score_result(host: &str, url: &str, title: &str, snippet: &str) -> (f32, bool) {
    let mut score = 1.0f32;
    if TRUSTED_HOSTS.iter().any(|t| host.contains(t)) {
        score += 3.0;
    }
    if host.ends_with(".edu") || host.ends_with(".gov") {
        score += 2.0;
    }
    // Свежесть в URL: 2024-2026
    let fresh = Regex::new(r"20(2[4-9])").unwrap().is_match(url);
    if fresh {
        score += 0.5;
    }
    if url.contains("?utm_") || host.contains("pinterest.") || host.contains("facebook.") {
        score -= 1.5;
    }
    if title.to_lowercase().contains("лучшие цены") || snippet.contains("Купить") {
        score -= 2.0;
    }
    if !snippet.is_empty() {
        score += 0.3;
    }
    (score, fresh)
}

// ------------------------------------------------------------------ публичный API

/// Полный поиск с кэшем: сниппеты + параллельная загрузка топ-страниц.
pub async fn search(query: &str) -> Result<Vec<SearchResult>, String> {
    if let Some(hit) = cache_get(query) {
        return Ok(hit);
    }
    let client = Client::builder().timeout(Duration::from_secs(12)).build().map_err(|e| e.to_string())?;
    let raw = ddg_search(&client, query).await?;
    let mut results: Vec<SearchResult> = raw
        .into_iter()
        .map(|(title, url, snippet)| {
            let host = host_of(&url);
            let (score, fresh) = score_result(&host, &url, &title, &snippet);
            SearchResult { title, url, snippet, content: String::new(), host, score, fresh }
        })
        .collect();

    // Параллельная загрузка топ-4 страниц.
    let top: Vec<String> = results.iter().take(FETCH_PAGES).map(|r| r.url.clone()).collect();
    let mut contents: HashMap<String, String> = HashMap::new();
    let mut tasks = tokio::task::JoinSet::new();
    for url in top {
        let client = client.clone();
        tasks.spawn(async move { (url.clone(), fetch_page(&client, &url).await) });
    }
    while let Some(Ok((url, content))) = tasks.join_next().await {
        if let Some(c) = content {
            contents.insert(url, c);
        }
    }
    for r in results.iter_mut() {
        if let Some(c) = contents.get(&r.url) {
            if !c.is_empty() {
                r.score += 1.0;
                r.content.clone_from(c);
            }
        }
    }
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    cache_put(query.to_string(), results.clone());
    Ok(results)
}

/// Контекст для LLM: компактный блок источников.
pub fn to_context(results: &[SearchResult], max_chars: usize) -> String {
    let mut out = String::new();
    for (i, r) in results.iter().take(5).enumerate() {
        let body = if r.content.is_empty() { &r.snippet } else { &r.content };
        let body: String = body.chars().take(1200).collect();
        out.push_str(&format!("{}. [{}] {}\nURL: {}\n{}\n\n", i + 1, r.host, r.title, r.url, body));
        if out.len() > max_chars {
            break;
        }
    }
    if out.len() > max_chars {
        out = out.chars().take(max_chars).into_iter().collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_strips_noise() {
        let html = r#"
        <html><body>
        <nav>Меню | О нас | Контакты</nav>
        <script>alert('x')</script>
        <article>
            <h1>Изобретение радио</h1>
            <p>Радио изобрёл <b>Маркони</b> в 1895 году.</p>
            <h2>Споры о приоритете</h2>
            <p>Попов продемонстрировал приёмник ранее.</p>
        </article>
        <footer>© 2026</footer>
        </body></html>"#;
        let md = markdown_from_html(html);
        assert!(md.contains("# Изобретение радио"));
        assert!(md.contains("## Споры"));
        assert!(md.contains("Маркони"));
        assert!(!md.contains("alert"));
        assert!(!md.contains("Меню"));
    }

    #[test]
    fn ddg_url_decode() {
        assert_eq!(
            decode_ddg_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fru.wikipedia.org%2Fwiki%2F%D0%A0%D0%B0%D0%B4%D0%B8%D0%BE&rut=abc"),
            "https://ru.wikipedia.org/wiki/Радио"
        );
    }

    #[test]
    fn ranking_prefers_trusted() {
        let (s1, _) = score_result("ru.wikipedia.org", "https://ru.wikipedia.org/wiki/X", "Заголовок", "текст");
        let (s2, _) = score_result("shop.example.com", "https://shop.example.com/buy?utm_source=x", "Лучшие цены", "Купить сейчас");
        assert!(s1 > s2);
    }

    #[test]
    fn cache_roundtrip() {
        cache_clear();
        let r = SearchResult {
            title: "t".into(), url: "https://x.ru".into(), snippet: "s".into(),
            content: "c".into(), host: "x.ru".into(), score: 1.0, fresh: false,
        };
        cache_put("тест".into(), vec![r]);
        assert!(cache_get("тест").is_some());
        assert!(cache_get("другое").is_none());
        cache_clear();
        assert!(cache_get("тест").is_none());
    }

    #[test]
    fn context_format() {
        let rs = vec![SearchResult {
            title: "Радио".into(), url: "https://ru.wikipedia.org/wiki/Радио".into(),
            snippet: "Радио — технология связи".into(), content: String::new(),
            host: "ru.wikipedia.org".into(), score: 2.0, fresh: false,
        }];
        let ctx = to_context(&rs, 4000);
        assert!(ctx.contains("[ru.wikipedia.org]"));
        assert!(ctx.contains("технология связи"));
    }
}
