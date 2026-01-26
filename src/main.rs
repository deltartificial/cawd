mod action;
mod app;
mod components;
mod tui;
mod utils;

use app::App;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "cawd")]
#[command(about = "A terminal code viewer with syntax highlighting")]
struct Args {
    /// Path to the directory or file to open
    #[arg(default_value = ".")]
    path: PathBuf,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let args = Args::parse();

    let path = if args.path.is_absolute() {
        args.path
    } else {
        std::env::current_dir()?.join(&args.path)
    };

    let mut terminal = tui::init()?;

    let result = App::new(path)?.run(&mut terminal);

    tui::restore()?;

    result
}
