use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use scraper::{Html, Selector};
use serde_json::json;

use crate::models::{ResourceKind, SnapshotMetadata, SnapshotPage};
use crate::snapshot::write_snapshot_pages_with_extra;

pub(crate) trait ResearchPaperRegistry {
    fn snapshot(
        &self,
        home: &Path,
        resource_id: &str,
        paper_id: &str,
        url: &str,
    ) -> Result<SnapshotMetadata>;
}

pub(crate) trait VersionedResearchPaperRegistry: ResearchPaperRegistry {
    fn version(&self, paper: &ResearchPaper) -> Option<String>;
}

pub(crate) struct ArxivRegistry;

impl ResearchPaperRegistry for ArxivRegistry {
    fn snapshot(
        &self,
        home: &Path,
        resource_id: &str,
        arxiv_id: &str,
        abs_url: &str,
    ) -> Result<SnapshotMetadata> {
        snapshot_arxiv(self, home, resource_id, arxiv_id, abs_url)
    }
}

impl VersionedResearchPaperRegistry for ArxivRegistry {
    fn version(&self, paper: &ResearchPaper) -> Option<String> {
        paper.version.clone()
    }
}

pub(crate) fn snapshot_arxiv(
    registry: &ArxivRegistry,
    home: &Path,
    resource_id: &str,
    arxiv_id: &str,
    abs_url: &str,
) -> Result<SnapshotMetadata> {
    eprintln!("fetching arXiv paper: {abs_url}");
    let client = reqwest::blocking::Client::builder()
        .user_agent("ctx/0.1 (+https://github.com/voiys/ctx; local paper indexer)")
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let abs_html = fetch_required(&client, abs_url)?;
    let paper = parse_abs_page(arxiv_id, abs_url, &abs_html)?;
    let mut pages = vec![SnapshotPage {
        url: abs_url.to_string(),
        content: paper.to_context_text(),
    }];

    let html_url = format!("https://arxiv.org/html/{arxiv_id}");
    if let Ok(full_html) = fetch_optional(&client, &html_url)
        && let Some(full_text) = arxiv_html_to_text(&full_html)
    {
        pages.push(SnapshotPage {
            url: html_url,
            content: full_text,
        });
    }

    write_snapshot_pages_with_extra(
        home,
        ResourceKind::ResearchPaper,
        resource_id,
        abs_url,
        pages,
        Some(json!({
            "registry": "arxiv",
            "version": registry.version(&paper),
        })),
    )
}

fn fetch_required(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let response = client.get(url).send()?.error_for_status()?;
    Ok(response.text()?)
}

fn fetch_optional(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let response = client.get(url).send()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(anyhow!("optional arXiv HTML page not found"));
    }
    Ok(response.error_for_status()?.text()?)
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ResearchPaper {
    id: String,
    version: Option<String>,
    title: String,
    authors: Vec<String>,
    date: Option<String>,
    pdf_url: Option<String>,
    abstract_text: String,
}

impl ResearchPaper {
    fn to_context_text(&self) -> String {
        let mut text = String::new();
        text.push_str("Research paper\n\nRegistry: arXiv\n");
        text.push_str("ID: ");
        text.push_str(&self.id);
        if let Some(version) = &self.version {
            text.push_str("\nVersion: ");
            text.push_str(version);
        }
        text.push_str("\nTitle: ");
        text.push_str(&self.title);
        if !self.authors.is_empty() {
            text.push_str("\nAuthors: ");
            text.push_str(&self.authors.join(", "));
        }
        if let Some(date) = &self.date {
            text.push_str("\nDate: ");
            text.push_str(date);
        }
        if let Some(pdf_url) = &self.pdf_url {
            text.push_str("\nPDF: ");
            text.push_str(pdf_url);
        }
        text.push_str("\n\nAbstract:\n");
        text.push_str(&self.abstract_text);
        text
    }
}

fn parse_abs_page(arxiv_id: &str, abs_url: &str, html: &str) -> Result<ResearchPaper> {
    let document = Html::parse_document(html);
    let title = meta_content(&document, "citation_title")
        .or_else(|| meta_property(&document, "og:title"))
        .with_context(|| format!("could not find title on arXiv page {abs_url}"))?;
    let abstract_text = meta_content(&document, "citation_abstract")
        .or_else(|| meta_property(&document, "og:description"))
        .with_context(|| format!("could not find abstract on arXiv page {abs_url}"))?;
    let authors = meta_contents(&document, "citation_author");
    let date = meta_content(&document, "citation_date");
    let pdf_url = meta_content(&document, "citation_pdf_url");
    let version =
        arxiv_version_from_abs_page(&document).or_else(|| arxiv_version_from_id(arxiv_id));
    Ok(ResearchPaper {
        id: arxiv_id.to_string(),
        version,
        title,
        authors,
        date,
        pdf_url,
        abstract_text,
    })
}

fn arxiv_version_from_abs_page(document: &Html) -> Option<String> {
    meta_property(document, "og:url")
        .as_deref()
        .and_then(arxiv_version_from_id)
}

fn arxiv_version_from_id(value: &str) -> Option<String> {
    let id = value
        .trim_end_matches(".pdf")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(value);
    let version_start = id.rfind('v')?;
    let version = &id[version_start..];
    let digits = &version[1..];
    (!digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| version.to_string())
}

fn arxiv_html_to_text(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for selector in [
        "article.ltx_document",
        "main article",
        "article",
        "main",
        "body",
    ] {
        let selector = Selector::parse(selector).expect("static selector");
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
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

fn meta_content(document: &Html, name: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[name=\"{name}\"]")).ok()?;
    document
        .select(&selector)
        .filter_map(|node| node.value().attr("content"))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn meta_contents(document: &Html, name: &str) -> Vec<String> {
    let Some(selector) = Selector::parse(&format!("meta[name=\"{name}\"]")).ok() else {
        return Vec::new();
    };
    document
        .select(&selector)
        .filter_map(|node| node.value().attr("content"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn meta_property(document: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[property=\"{property}\"]")).ok()?;
    document
        .select(&selector)
        .filter_map(|node| node.value().attr("content"))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_abs_page_citation_metadata() {
        let html = r#"
            <html><head>
                <meta name="citation_title" content="Attention Is All You Need" />
                <meta name="citation_author" content="Vaswani, Ashish" />
                <meta name="citation_author" content="Shazeer, Noam" />
                <meta name="citation_date" content="2017/06/12" />
                <meta name="citation_pdf_url" content="https://arxiv.org/pdf/1706.03762" />
                <meta name="citation_abstract" content="We propose the Transformer architecture." />
                <meta property="og:url" content="https://arxiv.org/abs/1706.03762v7" />
            </head></html>
        "#;

        let paper = parse_abs_page("1706.03762", "https://arxiv.org/abs/1706.03762", html).unwrap();

        assert_eq!(
            paper,
            ResearchPaper {
                id: "1706.03762".to_string(),
                version: Some("v7".to_string()),
                title: "Attention Is All You Need".to_string(),
                authors: vec!["Vaswani, Ashish".to_string(), "Shazeer, Noam".to_string()],
                date: Some("2017/06/12".to_string()),
                pdf_url: Some("https://arxiv.org/pdf/1706.03762".to_string()),
                abstract_text: "We propose the Transformer architecture.".to_string(),
            }
        );
        assert!(paper.to_context_text().contains("Transformer architecture"));
        assert!(paper.to_context_text().contains("Version: v7"));
    }

    #[test]
    fn extracts_arxiv_version_from_ids_and_urls() {
        assert_eq!(arxiv_version_from_id("1706.03762v7").as_deref(), Some("v7"));
        assert_eq!(
            arxiv_version_from_id("https://arxiv.org/pdf/1706.03762v7.pdf").as_deref(),
            Some("v7")
        );
        assert_eq!(arxiv_version_from_id("1706.03762"), None);
    }

    #[test]
    fn extracts_full_text_from_arxiv_html() {
        let text = arxiv_html_to_text(
            "<html><body><nav>Navigation chrome</nav><article class=\"ltx_document\"><h1>Paper</h1><p>First paragraph.</p><p>Second paragraph.</p></article></body></html>",
        )
        .unwrap();

        assert!(text.contains("Paper"));
        assert!(text.contains("First paragraph."));
        assert!(text.contains("Second paragraph."));
        assert!(!text.contains("Navigation chrome"));
    }
}
