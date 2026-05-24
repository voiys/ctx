use anyhow::{Result, anyhow, bail};
use url::Url;

use crate::models::{ResearchPaperRegistry, ResolvedInput};

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
            } else if matches!(url.host_str(), Some("arxiv.org" | "www.arxiv.org")) {
                let id = arxiv_id_from_url(&url)?;
                Ok(ResolvedInput::ResearchPaper {
                    registry: ResearchPaperRegistry::Arxiv,
                    url: format!("https://arxiv.org/abs/{id}"),
                    id,
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

fn arxiv_id_from_url(url: &Url) -> Result<String> {
    let segments = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    let Some((first, rest)) = segments.split_first() else {
        bail!("arXiv URL must include /abs/<id>, /pdf/<id>, or /html/<id>");
    };
    if !matches!(*first, "abs" | "pdf" | "html") || rest.is_empty() {
        bail!("arXiv URL must include /abs/<id>, /pdf/<id>, or /html/<id>");
    }
    let mut id = rest.join("/");
    if *first == "pdf" {
        id = id.trim_end_matches(".pdf").to_string();
    }
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '/' | '_'))
    {
        bail!("invalid arXiv identifier in URL");
    }
    Ok(id)
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

    #[test]
    fn classifies_arxiv_urls_as_papers() {
        for (input, expected_id) in [
            ("https://arxiv.org/abs/1706.03762", "1706.03762"),
            ("https://arxiv.org/pdf/1706.03762.pdf", "1706.03762"),
            ("https://arxiv.org/html/cs/9901001", "cs/9901001"),
        ] {
            let resolved = resolve_input(input).unwrap();
            match resolved {
                ResolvedInput::ResearchPaper { registry, id, url } => {
                    assert_eq!(registry, ResearchPaperRegistry::Arxiv);
                    assert_eq!(id, expected_id);
                    assert_eq!(url, format!("https://arxiv.org/abs/{expected_id}"));
                }
                _ => panic!("expected arxiv paper"),
            }
        }
    }
}
