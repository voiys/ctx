use std::collections::HashMap;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::util::content_hash;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MarkdownSection {
    pub(crate) section_index: usize,
    pub(crate) heading_path: Vec<String>,
    pub(crate) heading_level: usize,
    pub(crate) parent_section_index: Option<usize>,
    pub(crate) previous_section_index: Option<usize>,
    pub(crate) next_section_index: Option<usize>,
    pub(crate) anchor: Option<String>,
    pub(crate) markdown: String,
    pub(crate) plain_text: String,
    pub(crate) content_hash: String,
}

#[derive(Debug)]
struct HeadingMarker {
    start: usize,
    level: usize,
    title: String,
}

#[derive(Debug)]
struct HeadingDraft {
    start: usize,
    level: usize,
    title: String,
}

pub(crate) fn section_markdown(markdown: &str) -> Vec<MarkdownSection> {
    let normalized = markdown.replace("\r\n", "\n");
    let headings = heading_markers(&normalized);
    if headings.is_empty() {
        let trimmed = normalized.trim().to_string();
        if trimmed.is_empty() {
            return Vec::new();
        }
        return vec![MarkdownSection {
            section_index: 0,
            heading_path: Vec::new(),
            heading_level: 0,
            parent_section_index: None,
            previous_section_index: None,
            next_section_index: None,
            anchor: None,
            plain_text: plain_text(&trimmed),
            content_hash: content_hash(&trimmed),
            markdown: trimmed,
        }];
    }

    let mut sections = Vec::new();
    if let Some(first) = headings.first() {
        push_section(
            &mut sections,
            Vec::new(),
            0,
            None,
            None,
            section_slice(&normalized, 0, first.start),
        );
    }

    let mut heading_path: Vec<String> = Vec::new();
    let mut section_index_by_level: Vec<Option<usize>> = Vec::new();
    let mut anchor_counts = HashMap::new();

    for (heading_index, heading) in headings.iter().enumerate() {
        let end = headings
            .get(heading_index + 1)
            .map(|next| next.start)
            .unwrap_or(normalized.len());
        heading_path.truncate(heading.level.saturating_sub(1));
        while heading_path.len() < heading.level.saturating_sub(1) {
            heading_path.push(String::new());
        }
        if heading.level > 0 {
            if heading_path.len() == heading.level - 1 {
                heading_path.push(heading.title.clone());
            } else {
                heading_path[heading.level - 1] = heading.title.clone();
            }
        }
        section_index_by_level.truncate(heading.level.saturating_sub(1));
        let parent_section_index = last_defined(&section_index_by_level);
        let section_index = sections.len();
        let anchor = unique_anchor(&slugify(&heading.title), &mut anchor_counts);
        push_section(
            &mut sections,
            heading_path
                .iter()
                .filter(|part| !part.is_empty())
                .cloned()
                .collect(),
            heading.level,
            parent_section_index,
            Some(anchor),
            section_slice(&normalized, heading.start, end),
        );
        while section_index_by_level.len() < heading.level {
            section_index_by_level.push(None);
        }
        section_index_by_level[heading.level - 1] = Some(section_index);
    }

    let last_index = sections.len().saturating_sub(1);
    for (index, section) in sections.iter_mut().enumerate() {
        section.section_index = index;
        section.previous_section_index = index.checked_sub(1);
        section.next_section_index = (index < last_index).then_some(index + 1);
    }
    sections
}

fn heading_markers(markdown: &str) -> Vec<HeadingMarker> {
    let parser = Parser::new_ext(markdown, markdown_options());
    let mut headings = Vec::new();
    let mut current: Option<HeadingDraft> = None;

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some(HeadingDraft {
                    start: range.start,
                    level: heading_level(level),
                    title: String::new(),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(heading) = current.take() {
                    headings.push(HeadingMarker {
                        start: heading.start,
                        level: heading.level,
                        title: compact_whitespace(&heading.title),
                    });
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(heading) = &mut current {
                    heading.title.push_str(&text);
                    heading.title.push(' ');
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(heading) = &mut current {
                    heading.title.push(' ');
                }
            }
            _ => {}
        }
    }

    headings
}

fn push_section(
    sections: &mut Vec<MarkdownSection>,
    heading_path: Vec<String>,
    heading_level: usize,
    parent_section_index: Option<usize>,
    anchor: Option<String>,
    markdown: String,
) {
    if markdown.trim().is_empty() {
        return;
    }
    let plain_text = plain_text(&markdown);
    let content_hash = content_hash(&markdown);
    sections.push(MarkdownSection {
        section_index: sections.len(),
        heading_path,
        heading_level,
        parent_section_index,
        previous_section_index: None,
        next_section_index: None,
        anchor,
        markdown,
        plain_text,
        content_hash,
    });
}

pub(crate) fn plain_text(markdown: &str) -> String {
    let mut out = String::new();
    for event in Parser::new_ext(markdown, markdown_options()) {
        match event {
            Event::Text(text) | Event::Code(text) => {
                out.push_str(&text);
                out.push(' ');
            }
            Event::SoftBreak | Event::HardBreak | Event::Rule => out.push(' '),
            _ => {}
        }
    }
    compact_whitespace(&out)
}

fn section_slice(markdown: &str, start: usize, end: usize) -> String {
    markdown[start..end].trim().to_string()
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

fn heading_level(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn unique_anchor(anchor: &str, counts: &mut HashMap<String, usize>) -> String {
    let base = if anchor.is_empty() { "section" } else { anchor };
    let count = counts.entry(base.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base.to_string()
    } else {
        format!("{base}-{count}")
    }
}

fn last_defined(values: &[Option<usize>]) -> Option<usize> {
    values.iter().rev().find_map(|value| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_heading_hierarchy_and_section_links() {
        let sections = section_markdown(
            r#"# Guide

Intro.

## Setup

Configure it.

### Details

More detail.

## Finish

Ship it.
"#,
        );

        assert_eq!(
            sections
                .iter()
                .map(|section| section.heading_path.clone())
                .collect::<Vec<_>>(),
            vec![
                vec!["Guide"],
                vec!["Guide", "Setup"],
                vec!["Guide", "Setup", "Details"],
                vec!["Guide", "Finish"]
            ]
        );
        assert_eq!(
            sections
                .iter()
                .map(|section| section.heading_level)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 2]
        );
        assert_eq!(
            sections
                .iter()
                .map(|section| section.parent_section_index)
                .collect::<Vec<_>>(),
            vec![None, Some(0), Some(1), Some(0)]
        );
        assert_eq!(
            sections
                .iter()
                .map(|section| section.previous_section_index)
                .collect::<Vec<_>>(),
            vec![None, Some(0), Some(1), Some(2)]
        );
        assert_eq!(
            sections
                .iter()
                .map(|section| section.next_section_index)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3), None]
        );
    }

    #[test]
    fn keeps_numbered_workflow_steps_as_sibling_sections() {
        let sections = section_markdown(
            r#"# Review Client-Submitted Job

## Steps

### 1. Find the Job

Filter by In Review.

### 2. Review Job Details

Check title and salary.

### 3. Add Hiring Criteria

Add 3-5 must-have criteria.

### 4. Fill In Missing Information

Complete missing bounties.
"#,
        );

        assert_eq!(
            sections
                .iter()
                .filter_map(|section| section.heading_path.last().cloned())
                .collect::<Vec<_>>(),
            vec![
                "Review Client-Submitted Job",
                "Steps",
                "1. Find the Job",
                "2. Review Job Details",
                "3. Add Hiring Criteria",
                "4. Fill In Missing Information"
            ]
        );
        assert_eq!(
            sections[2..]
                .iter()
                .map(|section| section.parent_section_index)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(1), Some(1), Some(1)]
        );
    }

    #[test]
    fn ignores_headings_inside_fenced_code_blocks() {
        let sections = section_markdown(
            r#"# Troubleshooting

Before the example.

```md
# Not A Real Section
## Also Not Real
```

## Real Section

Actual docs.
"#,
        );

        assert_eq!(
            sections
                .iter()
                .map(|section| section.heading_path.clone())
                .collect::<Vec<_>>(),
            vec![
                vec!["Troubleshooting"],
                vec!["Troubleshooting", "Real Section"]
            ]
        );
        assert!(sections[0].markdown.contains("# Not A Real Section"));
    }

    #[test]
    fn keeps_tables_and_lists_inside_their_owning_section() {
        let sections = section_markdown(
            r#"# Payouts

## Readiness

| Check | Required |
| --- | --- |
| Active | Yes |

- Connected recipient
- No pending claim

## Trigger

Click send payout.
"#,
        );

        assert_eq!(
            sections
                .iter()
                .filter_map(|section| section.heading_path.last().cloned())
                .collect::<Vec<_>>(),
            vec!["Payouts", "Readiness", "Trigger"]
        );
        assert!(sections[1].markdown.contains("| Check"));
        assert!(sections[1].markdown.contains("- Connected recipient"));
        assert!(!sections[2].markdown.contains("Connected recipient"));
    }

    #[test]
    fn creates_a_fallback_section_for_unheaded_markdown() {
        let sections = section_markdown("Loose note with CTX_TOKEN.\n\nAnother paragraph.");

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading_path, Vec::<String>::new());
        assert_eq!(sections[0].heading_level, 0);
        assert_eq!(
            sections[0].plain_text,
            "Loose note with CTX_TOKEN. Another paragraph."
        );
    }

    #[test]
    fn creates_unique_anchors_for_duplicate_headings() {
        let sections = section_markdown(
            r#"# Setup

One.

# Setup

Two.
"#,
        );

        assert_eq!(
            sections
                .iter()
                .map(|section| section.anchor.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("setup"), Some("setup-2")]
        );
    }
}
