use pulldown_cmark::{Event, Options, Parser};

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

fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
