//! TUI module for cc-tail.
//!
//! Provides the interactive terminal user interface built on `ratatui`
//! and `crossterm`. The entry point is [`run_tui`], which takes over
//! the terminal, runs the event loop, and restores the terminal on exit
//! (including panics).

pub mod app;
pub mod event;
pub mod filter_overlay;
pub mod ui;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::project_path::detect_project_path;
use crate::session::{discover_sessions, resolve_session};
use crate::watcher::{self, WatcherEvent};
use app::App;
use event::{drain_log_entries, poll_crossterm_event, AppEvent};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How long to wait for a crossterm event before emitting a Tick.
const TICK_RATE: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Terminal setup / teardown
// ---------------------------------------------------------------------------

/// Set up the terminal for TUI mode: raw mode, alternate screen, mouse capture.
fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

/// Install a panic hook that restores the terminal before printing the
/// panic message. Without this, a panic leaves the terminal in raw mode
/// and the alternate screen, making the shell unusable.
fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Best-effort terminal restore; ignore errors.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));
}

// ---------------------------------------------------------------------------
// Signal handling
// ---------------------------------------------------------------------------

/// Set up a shared shutdown flag that is set to `true` when SIGINT or
/// SIGTERM is received from an external source (e.g. `kill -2`, `kill -15`).
///
/// Returns an `Arc<AtomicBool>` that the event loop checks each tick.
/// The flag is set by a background thread that listens for OS signals
/// using `tokio::signal` via a one-shot tokio runtime.
///
/// Note: When crossterm raw mode is active, Ctrl+C is intercepted as a
/// key event and does NOT generate SIGINT. This handler catches external
/// signals (e.g. `kill -2 <pid>`) that bypass the TUI input handling.
pub fn setup_signal_handler() -> Arc<AtomicBool> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    std::thread::spawn(move || {
        // Build a minimal tokio runtime just for signal listening.
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return, // Best-effort: if runtime fails, skip signal handling.
        };

        rt.block_on(async {
            // Wait for either SIGINT or SIGTERM.
            tokio::select! {
                _ = async {
                    if let Ok(mut s) = tokio::signal::unix::signal(
                        tokio::signal::unix::SignalKind::interrupt(),
                    ) {
                        s.recv().await;
                    }
                } => {}
                _ = async {
                    if let Ok(mut s) = tokio::signal::unix::signal(
                        tokio::signal::unix::SignalKind::terminate(),
                    ) {
                        s.recv().await;
                    }
                } => {}
            }
            shutdown_clone.store(true, Ordering::SeqCst);
        });
    });

    shutdown
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run the TUI application.
///
/// Takes over the terminal, enters the event loop, and restores the
/// terminal on exit. This is the main entry point called from `main()`.
pub fn run_tui(config: AppConfig) -> io::Result<()> {
    install_panic_hook();

    // Set up signal handler before entering raw mode so that external
    // SIGINT/SIGTERM triggers a clean shutdown.
    let shutdown_flag = setup_signal_handler();

    let mut terminal = setup_terminal()?;
    let mut app = App::new(config);

    // -- Session discovery and replay ----------------------------------------

    let mut watcher_rx: Option<mpsc::Receiver<WatcherEvent>> = None;
    let mut _watcher_handle: Option<watcher::WatcherHandle> = None;

    let cwd = std::env::current_dir().unwrap_or_default();
    match detect_project_path(&cwd, app.config.project.as_deref()) {
        Ok(project_dir) => {
            // Derive the display name from the project path.
            app.project_display_name = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string());
            app.project_path = Some(project_dir.clone());

            // Discover sessions.
            match discover_sessions(&project_dir, 50) {
                Ok(sessions) => {
                    if !sessions.is_empty() {
                        // Auto-select the most recent session (index 0).
                        let selected = resolve_session(&sessions, app.config.session.as_deref())
                            .ok()
                            .cloned()
                            .unwrap_or_else(|| sessions[0].clone());

                        app.sessions = sessions;
                        app.active_session_id = Some(selected.id.clone());

                        // Replay recent messages from the selected session.
                        app.replay_session_entries(&selected);

                        // Start the file watcher from where replay left off.
                        let offsets = app.replay_offsets.clone();
                        match watcher::start_watching(project_dir, app.config.verbose, 256, offsets)
                        {
                            Ok((rx, handle)) => {
                                watcher_rx = Some(rx);
                                _watcher_handle = Some(handle);
                            }
                            Err(e) => {
                                if app.config.verbose {
                                    eprintln!("cc-tail: watcher error: {}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if app.config.verbose {
                        eprintln!("cc-tail: session discovery error: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            if app.config.verbose {
                eprintln!("cc-tail: project detection error: {}", e);
            }
            app.status_message = Some(format!("No project detected: {}", e));
        }
    }

    let result = run_event_loop(&mut terminal, &mut app, &shutdown_flag, &mut watcher_rx);

    // Signal the watcher to shut down cleanly.
    if let Some(handle) = _watcher_handle.take() {
        handle.shutdown();
    }

    // Always restore terminal, even if the event loop returned an error.
    restore_terminal(&mut terminal)?;

    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Maximum number of watcher events to drain per tick.
const MAX_DRAIN_PER_TICK: usize = 200;

/// The core event loop: draw, poll, handle, repeat.
///
/// Checks the `shutdown_flag` each tick. When set by the signal handler,
/// the loop performs a force-quit to ensure clean terminal restoration.
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    shutdown_flag: &Arc<AtomicBool>,
    watcher_rx: &mut Option<mpsc::Receiver<WatcherEvent>>,
) -> io::Result<()> {
    loop {
        // Check for external signal (SIGINT/SIGTERM) — force quit.
        if shutdown_flag.load(Ordering::SeqCst) {
            app.should_quit = true;
            return Ok(());
        }

        // Draw only when state has changed.
        if app.needs_redraw {
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.needs_redraw = false;
        }

        // Poll for crossterm events
        if let Some(event) = poll_crossterm_event(TICK_RATE) {
            match event {
                AppEvent::Key(key) => app.on_key(key),
                AppEvent::Mouse(mouse) => app.on_mouse(mouse),
                AppEvent::Resize(_, _) => {
                    // ratatui handles resize automatically on next draw.
                    app.needs_redraw = true;
                }
                AppEvent::Tick | AppEvent::NewLogEntry(_) | AppEvent::NewFileDetected(_) => {}
            }
        }

        // Drain watcher events (non-blocking).
        if let Some(ref mut rx) = watcher_rx {
            let watcher_events = drain_log_entries(rx, MAX_DRAIN_PER_TICK);
            for evt in watcher_events {
                match evt {
                    AppEvent::NewLogEntry(entry) => app.on_new_log_entry(*entry),
                    AppEvent::NewFileDetected(path) => app.on_new_file_detected(path),
                    _ => {}
                }
            }
        }

        // Check quit
        if app.should_quit {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_signal_handler_returns_false_initially() {
        let flag = setup_signal_handler();
        // The flag should be false immediately after creation.
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_multiple_signal_handler_calls_independent() {
        // Each call to setup_signal_handler returns an independent flag.
        let flag1 = setup_signal_handler();
        let flag2 = setup_signal_handler();

        assert!(!flag1.load(Ordering::SeqCst));
        assert!(!flag2.load(Ordering::SeqCst));

        // Setting one should not affect the other.
        flag1.store(true, Ordering::SeqCst);
        assert!(flag1.load(Ordering::SeqCst));
        assert!(!flag2.load(Ordering::SeqCst));
    }
}
