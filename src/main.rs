//! # cawd - Code Aware Workspace Display
//!
//! A terminal-based file explorer with syntax highlighting designed for code reading.
//! Use alongside AI coding assistants to visually verify generated code in real-time.

#![warn(clippy::all)]

mod action;
mod app;
mod components;
mod tui;
mod utils;

use app::App;
use clap::Parser;
use std::path::PathBuf;

/// Command-line arguments for cawd.
#[derive(Parser, Debug)]
#[command(name = "cawd")]
#[command(about = "A terminal code viewer with syntax highlighting")]
struct Args {
    /// Path to the directory or file to open.
    /// Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,
}

/// Application entry point.
///
/// Initializes the terminal UI, runs the main application loop,
/// and ensures proper cleanup on exit.
///
/// # Returns
///
/// Returns `Ok(())` on successful execution, or an error if
/// terminal initialization or application execution fails.
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
