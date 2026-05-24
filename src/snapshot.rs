use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::crawl::crawl_docs;
use crate::models::{ResourceKind, SnapshotMetadata, SnapshotPage};
use crate::util::{content_hash, timestamp};

pub(crate) fn snapshot_docs(
    home: &Path,
    resource_id: &str,
    url: &str,
    max_pages: usize,
    concurrency: usize,
) -> Result<SnapshotMetadata> {
    eprintln!("crawling docs: {url}");
    let pages = crawl_docs(url, max_pages, concurrency)?;
    write_snapshot_pages(home, ResourceKind::Docs, resource_id, url, pages)
}

pub(crate) fn snapshot_notes(
    home: &Path,
    resource_id: &str,
    url: &str,
    path: &Path,
) -> Result<SnapshotMetadata> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read notes file {}", path.display()))?;
    write_snapshot_pages(
        home,
        ResourceKind::Notes,
        resource_id,
        url,
        vec![SnapshotPage {
            url: url.to_string(),
            content: text,
        }],
    )
}

pub(crate) fn write_snapshot_pages(
    home: &Path,
    kind: ResourceKind,
    resource_id: &str,
    source_url: &str,
    pages: Vec<SnapshotPage>,
) -> Result<SnapshotMetadata> {
    let mut hash_input = String::new();
    let mut combined = String::new();
    for page in &pages {
        hash_input.push_str(&page.url);
        hash_input.push('\n');
        hash_input.push_str(&page.content);
        hash_input.push('\n');
        combined.push_str("# ");
        combined.push_str(&page.url);
        combined.push_str("\n\n");
        combined.push_str(&page.content);
        combined.push_str("\n\n");
    }
    let hash = content_hash(&hash_input);
    let fetched_at = timestamp();
    let snapshot_id = format!("{}-{}", fetched_at.replace([':', '-'], ""), &hash[..12]);
    let root = match kind {
        ResourceKind::Docs => home.join("docs"),
        ResourceKind::Notes => home.join("notes"),
        ResourceKind::Arxiv => home.join("arxiv"),
        ResourceKind::Source => bail!("source snapshots are not supported"),
    };
    let path = root.join(resource_id).join(&snapshot_id);
    fs::create_dir_all(&path)?;
    fs::write(path.join("content.txt"), &combined)?;
    fs::write(
        path.join("pages.json"),
        serde_json::to_string_pretty(&pages)?,
    )?;
    let metadata = SnapshotMetadata {
        snapshot_id,
        fetched_at,
        source_url: source_url.to_string(),
        content_hash: format!("sha256:{hash}"),
        page_count: pages.len(),
        path: path.display().to_string(),
    };
    fs::write(
        path.join("snapshot.json"),
        serde_json::to_string_pretty(&metadata)?,
    )?;
    Ok(metadata)
}
