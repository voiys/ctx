use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

use crate::models::{Manifest, Resource, ResourceKind};

pub(crate) fn read_manifest(path: &Path) -> Result<Manifest> {
    let raw = fs::read_to_string(path)
        .map_err(|error| anyhow!("failed to read manifest at {}: {error}", path.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

pub(crate) fn write_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let manifest = portable_manifest(manifest);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&manifest)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn portable_manifest(manifest: &Manifest) -> Manifest {
    Manifest {
        version: manifest.version,
        defaults: crate::models::Defaults {
            top_k: manifest.defaults.top_k,
            budget_tokens: manifest.defaults.budget_tokens,
        },
        resources: manifest
            .resources
            .iter()
            .cloned()
            .map(|mut resource| {
                resource.local_path = None;
                resource
            })
            .collect(),
    }
}

pub(crate) fn upsert_manifest_resource(manifest: &mut Manifest, mut resource: Resource) {
    if manifest.version == 0 {
        manifest.version = 1;
    }
    if let Some(existing) = manifest
        .resources
        .iter_mut()
        .find(|existing| existing.label == resource.label || existing.url == resource.url)
    {
        resource.created_at = existing.created_at.clone();
        *existing = resource;
    } else {
        manifest.resources.push(resource);
    }
}

pub(crate) fn find_manifest_resource<'a>(
    manifest: &'a Manifest,
    target: &str,
) -> Result<&'a Resource> {
    manifest
        .resources
        .iter()
        .find(|resource| {
            resource.label == target || resource.url == target || resource.id == target
        })
        .ok_or_else(|| anyhow!("resource not found: {target}"))
}

pub(crate) fn find_manifest_resource_index(manifest: &Manifest, target: &str) -> Result<usize> {
    manifest
        .resources
        .iter()
        .position(|resource| {
            resource.label == target || resource.url == target || resource.id == target
        })
        .ok_or_else(|| anyhow!("resource not found: {target}"))
}

pub(crate) fn allowed_resource_ids(
    manifest: &Manifest,
    label: Option<&str>,
    kind: Option<ResourceKind>,
) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for resource in &manifest.resources {
        if matches!(resource.kind, ResourceKind::Source) {
            continue;
        }
        if let Some(label) = label
            && resource.label != label
        {
            continue;
        }
        if let Some(kind) = kind
            && resource.kind != kind
        {
            continue;
        }
        out.insert(resource.id.clone());
    }
    if label.is_some() && out.is_empty() {
        bail!("no queryable resource matched label");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Defaults, Resource};

    #[test]
    fn queryable_resources_never_include_sources() {
        let manifest = Manifest {
            version: 1,
            defaults: Defaults::default(),
            resources: vec![
                test_resource("source-id", "source", ResourceKind::Source),
                test_resource("docs-id", "docs", ResourceKind::Docs),
                test_resource("notes-id", "notes", ResourceKind::Notes),
                test_resource(
                    "research-paper-id",
                    "research-paper",
                    ResourceKind::ResearchPaper,
                ),
            ],
        };

        let all = allowed_resource_ids(&manifest, None, None).unwrap();
        assert_eq!(
            all,
            BTreeSet::from([
                "research-paper-id".into(),
                "docs-id".into(),
                "notes-id".into()
            ])
        );

        let source_label = allowed_resource_ids(&manifest, Some("source"), None);
        assert!(source_label.is_err());

        let docs = allowed_resource_ids(&manifest, None, Some(ResourceKind::Docs)).unwrap();
        assert_eq!(docs, BTreeSet::from(["docs-id".into()]));

        let papers =
            allowed_resource_ids(&manifest, None, Some(ResourceKind::ResearchPaper)).unwrap();
        assert_eq!(papers, BTreeSet::from(["research-paper-id".into()]));
    }

    fn test_resource(id: &str, label: &str, kind: ResourceKind) -> Resource {
        Resource {
            id: id.to_string(),
            label: label.to_string(),
            kind,
            url: format!("https://example.com/{label}"),
            reason: None,
            current: "current".to_string(),
            local_path: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }
}
