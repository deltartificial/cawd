//! Terminal User Interface initialization and cleanup utilities.

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::panic;

/// Type alias for the terminal with crossterm backend.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Initializes the terminal for TUI mode.
///
/// Enables raw mode for direct keyboard input handling and switches
/// to the alternate screen buffer to preserve the user's terminal content.
///
/// # Returns
///
/// Returns a configured `Terminal` instance ready for rendering,
/// or an error if terminal setup fails.
///
/// # Example
///
/// ```no_run
/// let mut terminal = tui::init()?;
/// // Use terminal for rendering...
/// tui::restore()?;
/// ```
pub fn init() -> color_eyre::Result<Tui> {
    // Set up panic hook to restore terminal on panic
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore();
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(io::stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restores the terminal to its original state.
///
/// Disables raw mode and returns to the main screen buffer.
/// Should always be called before the application exits to ensure
/// the user's terminal is left in a usable state.
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if restoration fails.
pub fn restore() -> color_eyre::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
