//! Terminal User Interface initialization and cleanup utilities.

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, Stdout, Write},
    panic,
};

/// Type alias for the terminal with crossterm backend.
pub(crate) type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Enables mouse reporting for button press/release (1000), button-drag motion
/// (1002), and SGR extended coordinates (1006).
///
/// This deliberately omits any-motion tracking (`?1003h`, which crossterm's
/// `EnableMouseCapture` turns on): that mode reports every mouse movement even
/// with no button held, flooding the event loop and hurting input fluidity.
/// Drag selection only needs 1002 (motion while a button is pressed).
const ENABLE_MOUSE: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1006h";

/// Disables the mouse modes enabled by [`ENABLE_MOUSE`], in reverse order.
const DISABLE_MOUSE: &str = "\x1b[?1006l\x1b[?1002l\x1b[?1000l";

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
/// ```text
/// let mut terminal = tui::init()?;
/// // Use terminal for rendering...
/// tui::restore()?;
/// ```
pub(crate) fn init() -> color_eyre::Result<Tui> {
    // Set up panic hook to restore terminal on panic
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        _ = restore();
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    stdout.write_all(ENABLE_MOUSE.as_bytes())?;
    stdout.flush()?;
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
pub(crate) fn restore() -> color_eyre::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(DISABLE_MOUSE.as_bytes())?;
    stdout.flush()?;
    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen)?;
    Ok(())
}
