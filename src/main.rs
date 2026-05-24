mod agents;
mod app;
mod constants;
mod context;
mod crawl;
mod embeddings;
mod input;
mod install;
mod manifest;
mod models;
mod output;
mod retrieve;
mod snapshot;
mod source;
mod storage;
mod util;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
