mod constants;
mod models;
mod output;
mod util;
mod manifest;
mod input;
mod crawl;
mod embeddings;
mod source;
mod snapshot;
mod storage;
mod retrieve;
mod context;
mod agents;
mod install;
mod app;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
