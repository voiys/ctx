use std::collections::{BTreeSet, HashSet};
use std::io::Read;
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use rayon::prelude::*;
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use scraper::{Html, Selector};
use url::Url;

use crate::models::SnapshotPage;

const MAX_DOC_BODY_BYTES: usize = 5 * 1024 * 1024;
const MAX_RETRY_AFTER_SECS: u64 = 10;

pub(crate) fn crawl_docs(
    seed: &str,
    max_pages: usize,
    concurrency: usize,
) -> Result<Vec<SnapshotPage>> {
    let seed_url = Url::parse(seed)?;
    let max_pages = max_pages.max(1);
    let concurrency = concurrency.max(1);
    let client = reqwest::blocking::Client::builder()
        .user_agent("ctx/0.1 (+https://github.com/voiys/ctx; local docs indexer)")
        .timeout(std::time::Duration::from_secs(20))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(concurrency)
        .build()?;
    let mut seen = HashSet::new();
    let mut frontier = Vec::new();
    let mut pages = Vec::new();
    for llms_url in llms_candidate_urls(&seed_url) {
        let canonical = canonical_url(&llms_url);
        if !seen.insert(canonical) {
            continue;
        }
        let Ok(fetched_page) = fetch_doc_page(&client, &llms_url) else {
            continue;
        };
        for link in &fetched_page.links {
            if is_crawlable_llms_url(&llms_url, link) {
                let canonical = canonical_url(link);
                if seen.insert(canonical.clone()) {
                    frontier.push(link.clone());
                }
            }
        }
        pages.push(SnapshotPage {
            url: fetched_page.url,
            content: fetched_page.text,
        });
        if pages.len() >= max_pages {
            return Ok(pages);
        }
        break;
    }

    if !is_llms_txt_url(&seed_url) && seen.insert(canonical_url(&seed_url)) {
        frontier.push(seed_url.clone());
    }

    while !frontier.is_empty() && pages.len() < max_pages {
        let remaining = max_pages - pages.len();
        let batch = frontier.into_iter().take(remaining).collect::<Vec<_>>();
        let fetched = pool.install(|| {
            batch
                .par_iter()
                .filter_map(|url| fetch_doc_page(&client, url).ok())
                .collect::<Vec<_>>()
        });
        let mut next = BTreeSet::new();
        for fetched_page in fetched {
            let page_url = fetched_page.url.clone();
            for link in &fetched_page.links {
                if is_crawlable_doc_url(&seed_url, link) {
                    let canonical = canonical_url(link);
                    if seen.insert(canonical.clone()) {
                        next.insert(canonical);
                    }
                }
            }
            pages.push(SnapshotPage {
                url: page_url,
                content: fetched_page.text,
            });
            if pages.len() >= max_pages {
                break;
            }
        }
        frontier = next
            .into_iter()
            .filter_map(|value| Url::parse(&value).ok())
            .collect();
    }

    if pages.is_empty() {
        bail!("no crawlable docs pages were fetched from {seed}");
    }
    Ok(pages)
}

struct FetchedDocPage {
    url: String,
    text: String,
    links: Vec<Url>,
}

fn fetch_doc_page(client: &reqwest::blocking::Client, url: &Url) -> Result<FetchedDocPage> {
    eprintln!("fetching docs page: {url}");
    let response = send_with_backoff(client, url)?;
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let body = read_capped_body(response, MAX_DOC_BODY_BYTES)?;
    let links = page_links(url, &body);
    let text = if content_type.contains("html") || looks_like_html(&body) {
        html_to_text(&body)
    } else {
        body
    };
    Ok(FetchedDocPage {
        url: canonical_url(url),
        text,
        links,
    })
}

fn send_with_backoff(
    client: &reqwest::blocking::Client,
    url: &Url,
) -> Result<reqwest::blocking::Response> {
    let mut delay = Duration::from_millis(500);
    for attempt in 0..3 {
        let response = client.get(url.clone()).send()?;
        if !matches!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ) {
            return Ok(response.error_for_status()?);
        }
        if attempt == 2 {
            return Ok(response.error_for_status()?);
        }
        let retry_after = capped_retry_after(response.headers().get(RETRY_AFTER), delay);
        thread::sleep(retry_after);
        delay *= 2;
    }
    unreachable!("retry loop returns or errors")
}

fn capped_retry_after(value: Option<&HeaderValue>, fallback: Duration) -> Duration {
    value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(fallback)
        .min(Duration::from_secs(MAX_RETRY_AFTER_SECS))
}

fn read_capped_body<R: Read>(reader: R, max_bytes: usize) -> Result<String> {
    let mut bytes = Vec::new();
    reader.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        bail!("docs page exceeded {max_bytes} byte limit");
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn page_links(base: &Url, body: &str) -> Vec<Url> {
    let mut links = html_links(base, body);
    links.extend(text_links(base, body));
    dedupe_urls(links)
}

fn html_links(base: &Url, html: &str) -> Vec<Url> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href]").expect("static selector");
    document
        .select(&selector)
        .filter_map(|node| node.value().attr("href"))
        .filter_map(|href| base.join(href).ok())
        .map(strip_fragment_and_query)
        .collect()
}

fn text_links(base: &Url, text: &str) -> Vec<Url> {
    let mut out = Vec::new();
    for href in markdown_link_targets(text).chain(bare_url_targets(text)) {
        if let Ok(url) = base.join(&href) {
            out.push(strip_fragment_and_query(url));
        }
    }
    out
}

fn markdown_link_targets(text: &str) -> impl Iterator<Item = String> + '_ {
    text.match_indices("](").filter_map(|(start, _)| {
        let href_start = start + 2;
        let href_end = text[href_start..].find(')')? + href_start;
        Some(text[href_start..href_end].trim().to_string())
    })
}

fn bare_url_targets(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split_whitespace().filter_map(|part| {
        let trimmed = part.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>' | ',' | '.'
            )
        });
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            Some(trimmed.to_string())
        } else {
            None
        }
    })
}

fn dedupe_urls(urls: Vec<Url>) -> Vec<Url> {
    let mut seen = HashSet::new();
    urls.into_iter()
        .filter(|url| seen.insert(canonical_url(url)))
        .collect()
}

pub(crate) fn is_crawlable_doc_url(seed: &Url, candidate: &Url) -> bool {
    if candidate.scheme() != seed.scheme()
        || candidate.host_str() != seed.host_str()
        || candidate.port_or_known_default() != seed.port_or_known_default()
    {
        return false;
    }
    let seed_path = normalized_crawl_root(seed.path());
    if !candidate.path().starts_with(&seed_path) {
        return false;
    }
    !looks_like_asset(candidate.path())
}

fn is_crawlable_llms_url(llms_url: &Url, candidate: &Url) -> bool {
    if candidate.scheme() != llms_url.scheme()
        || candidate.host_str() != llms_url.host_str()
        || candidate.port_or_known_default() != llms_url.port_or_known_default()
    {
        return false;
    }
    let Some(prefix) = llms_parent_path(llms_url) else {
        return false;
    };
    candidate.path().starts_with(&prefix) && !looks_like_asset(candidate.path())
}

fn llms_candidate_urls(seed: &Url) -> Vec<Url> {
    if is_llms_txt_url(seed) {
        return vec![strip_fragment_and_query(seed.clone())];
    }
    let mut candidates = Vec::new();
    if let Ok(url) = Url::parse(&format!("{}/llms.txt", seed.as_str().trim_end_matches('/'))) {
        candidates.push(url);
    }
    if let Ok(url) = seed.join("llms.txt") {
        candidates.push(url);
    }
    let mut origin = seed.clone();
    origin.set_path("/llms.txt");
    origin.set_query(None);
    origin.set_fragment(None);
    candidates.push(origin);
    dedupe_urls(candidates)
}

fn is_llms_txt_url(url: &Url) -> bool {
    url.path().trim_end_matches('/').ends_with("/llms.txt")
}

fn llms_parent_path(url: &Url) -> Option<String> {
    let path = url.path();
    let parent = path.rsplit_once('/')?.0;
    if parent.is_empty() {
        Some("/".to_string())
    } else {
        Some(format!("{}/", parent.trim_end_matches('/')))
    }
}

fn normalized_crawl_root(path: &str) -> String {
    if path == "/" || path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    }
}

fn looks_like_asset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".css", ".js", ".mjs", ".map",
        ".pdf", ".zip", ".tar", ".gz", ".mp4", ".webm", ".woff", ".woff2", ".ttf",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn strip_fragment_and_query(mut url: Url) -> Url {
    url.set_fragment(None);
    url.set_query(None);
    url
}

fn canonical_url(url: &Url) -> String {
    strip_fragment_and_query(url.clone()).to_string()
}

fn html_to_text(html: &str) -> String {
    let document = Html::parse_document(html);
    let selector = Selector::parse("main, article, body").expect("static selector");
    let mut text = String::new();
    for node in document.select(&selector).take(1) {
        for part in node.text() {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                text.push_str(trimmed);
                text.push('\n');
            }
        }
    }
    if text.trim().is_empty() {
        html.to_string()
    } else {
        text
    }
}

fn looks_like_html(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with("<!DOCTYPE html")
        || trimmed.starts_with("<html")
        || trimmed.contains("<body")
        || trimmed.contains("<main")
        || trimmed.contains("<article")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crawl_filter_stays_under_seed_path_and_skips_assets() {
        let seed = Url::parse("https://example.com/docs/guide/").unwrap();
        let child = Url::parse("https://example.com/docs/guide/install").unwrap();
        let sibling = Url::parse("https://example.com/docs/api").unwrap();
        let other_host = Url::parse("https://other.example.com/docs/guide/install").unwrap();
        let asset = Url::parse("https://example.com/docs/guide/logo.svg").unwrap();

        assert!(is_crawlable_doc_url(&seed, &child));
        assert!(!is_crawlable_doc_url(&seed, &sibling));
        assert!(!is_crawlable_doc_url(&seed, &other_host));
        assert!(!is_crawlable_doc_url(&seed, &asset));
    }

    #[test]
    fn llms_candidates_cover_directory_and_origin_conventions() {
        let seed = Url::parse("https://example.com/docs/overview").unwrap();
        let candidates = llms_candidate_urls(&seed)
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>();

        assert!(candidates.contains(&"https://example.com/docs/overview/llms.txt".to_string()));
        assert!(candidates.contains(&"https://example.com/docs/llms.txt".to_string()));
        assert!(candidates.contains(&"https://example.com/llms.txt".to_string()));
    }

    #[test]
    fn explicit_llms_seed_does_not_probe_extra_llms_variants() {
        let seed = Url::parse("https://example.com/docs/llms.txt").unwrap();
        let candidates = llms_candidate_urls(&seed)
            .into_iter()
            .map(|url| url.to_string())
            .collect::<Vec<_>>();

        assert_eq!(candidates, vec!["https://example.com/docs/llms.txt"]);
    }

    #[test]
    fn llms_links_can_discover_sibling_docs_pages() {
        let llms = Url::parse("https://example.com/docs/llms.txt").unwrap();
        let child = Url::parse("https://example.com/docs/guide.md").unwrap();
        let sibling_root = Url::parse("https://example.com/api/reference.md").unwrap();
        let asset = Url::parse("https://example.com/docs/logo.svg").unwrap();

        assert!(is_crawlable_llms_url(&llms, &child));
        assert!(!is_crawlable_llms_url(&llms, &sibling_root));
        assert!(!is_crawlable_llms_url(&llms, &asset));
    }

    #[test]
    fn text_links_extract_markdown_and_bare_urls() {
        let base = Url::parse("https://example.com/docs/llms.txt").unwrap();
        let links = text_links(
            &base,
            "- [Guide](guide.md)\nSee https://example.com/docs/api.md.",
        )
        .into_iter()
        .map(|url| url.to_string())
        .collect::<Vec<_>>();

        assert!(links.contains(&"https://example.com/docs/guide.md".to_string()));
        assert!(links.contains(&"https://example.com/docs/api.md".to_string()));
    }

    #[test]
    fn retry_after_is_capped() {
        let value = HeaderValue::from_static("999");

        assert_eq!(
            capped_retry_after(Some(&value), Duration::from_millis(500)),
            Duration::from_secs(MAX_RETRY_AFTER_SECS)
        );
    }

    #[test]
    fn capped_body_rejects_oversized_input() {
        let body = "abcdef";

        assert!(read_capped_body(body.as_bytes(), 5).is_err());
        assert_eq!(read_capped_body(body.as_bytes(), 6).unwrap(), body);
    }
}
