use clap::Parser;
use organise::Cli;

mod app;

fn main() {
    if let Err(e) = app::main(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
