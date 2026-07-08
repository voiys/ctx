mod agents;
mod app;
mod arxiv;
mod constants;
mod context;
mod crawl;
mod embeddings;
mod input;
mod install;
mod jobs;
mod journal;
mod l1;
mod l2;
mod l3;
mod manifest;
mod markdown;
mod memory;
mod models;
mod output;
mod retrieve;
#[allow(dead_code)]
mod sanitize;
mod snapshot;
mod source;
mod storage;
#[allow(dead_code)]
mod time;
mod util;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
