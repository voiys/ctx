use std::collections::{BTreeSet, HashSet};

use anyhow::{Result, bail};
use rayon::prelude::*;
use scraper::{Html, Selector};
use url::Url;

use crate::models::SnapshotPage;

pub(crate) fn crawl_docs(
    seed: &str,
    max_pages: usize,
    concurrency: usize,
) -> Result<Vec<SnapshotPage>> {
    let seed_url = Url::parse(seed)?;
    let max_pages = max_pages.max(1);
    let concurrency = concurrency.max(1);
    let client = reqwest::blocking::Client::builder()
        .user_agent("ctx/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(concurrency)
        .build()?;
    let mut seen = HashSet::from([canonical_url(&seed_url)]);
    let mut frontier = vec![seed_url.clone()];
    let mut pages = Vec::new();

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
    let html = client.get(url.clone()).send()?.error_for_status()?.text()?;
    let links = html_links(url, &html);
    Ok(FetchedDocPage {
        url: canonical_url(url),
        text: html_to_text(&html),
        links,
    })
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
}
