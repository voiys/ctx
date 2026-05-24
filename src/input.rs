use anyhow::{Result, anyhow, bail};
use url::Url;

use crate::models::ResolvedInput;

pub(crate) fn resolve_input(input: &str) -> Result<ResolvedInput> {
    let url = Url::parse(input).map_err(|_| anyhow!("ctx add requires an absolute URL"))?;
    match url.scheme() {
        "http" | "https" => {
            if url.host_str() == Some("github.com") {
                let segments = url
                    .path_segments()
                    .map(|segments| segments.collect::<Vec<_>>())
                    .unwrap_or_default();
                if segments.len() < 2 {
                    bail!("GitHub URL must include owner and repo");
                }
                let owner = segments[0].to_string();
                let repo = segments[1].trim_end_matches(".git").to_string();
                let requested_ref = if segments.get(2) == Some(&"tree") {
                    segments.get(3).map(|value| value.to_string())
                } else {
                    None
                };
                Ok(ResolvedInput::GithubSource {
                    owner,
                    repo,
                    requested_ref,
                    clone_url: format!("https://github.com/{}/{}.git", segments[0], segments[1]),
                })
            } else {
                Ok(ResolvedInput::Docs {
                    url: url.to_string(),
                })
            }
        }
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|_| anyhow!("file URL must point to an absolute local path"))?;
            Ok(ResolvedInput::Notes {
                url: url.to_string(),
                path,
            })
        }
        scheme => bail!("unsupported URL scheme: {scheme}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_github_as_source() {
        let resolved = resolve_input("https://github.com/owner/repo/tree/v1").unwrap();
        match resolved {
            ResolvedInput::GithubSource {
                owner,
                repo,
                requested_ref,
                ..
            } => {
                assert_eq!(owner, "owner");
                assert_eq!(repo, "repo");
                assert_eq!(requested_ref.as_deref(), Some("v1"));
            }
            _ => panic!("expected github source"),
        }
    }

    #[test]
    fn rejects_bare_paths() {
        assert!(resolve_input("/tmp/notes.md").is_err());
    }

    #[test]
    fn resolver_accepts_only_absolute_urls() {
        for input in ["owner/repo", "npm:zod", "docs/index.html", "../notes.md"] {
            assert!(resolve_input(input).is_err(), "{input} should be rejected");
        }
        assert!(matches!(
            resolve_input("https://example.com/docs").unwrap(),
            ResolvedInput::Docs { .. }
        ));
    }
}
