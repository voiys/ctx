mod constants;
mod models;
mod output;
mod util;
mod manifest;
mod input;
mod app;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
