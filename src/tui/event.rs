//! Event handling for the TUI event loop.
//!
//! Wraps crossterm key/resize events and log-entry events from the
//! watcher into a single [`AppEvent`] enum that the main loop can
//! `match` on.

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::log_entry::LogEntry;

// ---------------------------------------------------------------------------
// AppEvent
// ---------------------------------------------------------------------------

/// Events consumed by the TUI event loop.
#[derive(Debug)]
pub enum AppEvent {
    /// A key was pressed.
    Key(KeyEvent),
    /// A mouse event occurred (scroll, click, etc.).
    Mouse(MouseEvent),
    /// The terminal was resized.
    #[allow(dead_code)]
    Resize(u16, u16),
    /// A new log entry arrived from the file watcher.
    NewLogEntry(Box<LogEntry>),
    /// A new JSONL file was detected by the watcher.
    NewFileDetected(PathBuf),
    /// A periodic tick (used for UI refresh, cursor blink, etc.).
    #[allow(dead_code)]
    Tick,
}

// ---------------------------------------------------------------------------
// Event polling
// ---------------------------------------------------------------------------

/// Poll for the next crossterm event with the given timeout.
///
/// Returns `Some(AppEvent)` if an event was available, `None` on timeout.
/// This is a blocking call intended to be run from the main thread.
pub fn poll_crossterm_event(timeout: Duration) -> Option<AppEvent> {
    if event::poll(timeout).ok()? {
        match event::read().ok()? {
            CrosstermEvent::Key(key) => Some(AppEvent::Key(key)),
            CrosstermEvent::Mouse(mouse) => Some(AppEvent::Mouse(mouse)),
            CrosstermEvent::Resize(w, h) => Some(AppEvent::Resize(w, h)),
            _ => None,
        }
    } else {
        None
    }
}

/// Drain pending log entries from the watcher channel, up to `max_per_tick`.
///
/// Returns the entries drained. Stops as soon as `try_recv()` returns
/// `Err` (empty or disconnected), so this never blocks.
pub fn drain_log_entries(
    rx: &mut mpsc::Receiver<crate::watcher::WatcherEvent>,
    max_per_tick: usize,
) -> Vec<AppEvent> {
    let mut events = Vec::new();

    for _ in 0..max_per_tick {
        match rx.try_recv() {
            Ok(crate::watcher::WatcherEvent::NewEntry { entry, .. }) => {
                events.push(AppEvent::NewLogEntry(entry));
            }
            Ok(crate::watcher::WatcherEvent::NewFileDetected { path }) => {
                events.push(AppEvent::NewFileDetected(path));
            }
            Ok(_) => {
                // Error — skip for now
            }
            Err(_) => break,
        }
    }

    events
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_entry::{parse_jsonl_line, EntryType};
    use crate::watcher::WatcherEvent;
    use std::path::PathBuf;

    // -- drain_log_entries tests ---------------------------------------------

    #[tokio::test]
    async fn test_drain_empty_channel() {
        let (_tx, mut rx) = mpsc::channel::<WatcherEvent>(16);
        let events = drain_log_entries(&mut rx, 100);
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_drain_with_entries() {
        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);

        let entry1 = parse_jsonl_line(r#"{"type": "user", "sessionId": "s1"}"#).unwrap();
        let entry2 = parse_jsonl_line(r#"{"type": "assistant", "sessionId": "s1"}"#).unwrap();

        tx.send(WatcherEvent::NewEntry {
            source: PathBuf::from("/fake/s1.jsonl"),
            entry: Box::new(entry1),
        })
        .await
        .unwrap();
        tx.send(WatcherEvent::NewEntry {
            source: PathBuf::from("/fake/s1.jsonl"),
            entry: Box::new(entry2),
        })
        .await
        .unwrap();

        let events = drain_log_entries(&mut rx, 100);
        assert_eq!(events.len(), 2);

        // Verify they are NewLogEntry variants
        for event in &events {
            assert!(matches!(event, AppEvent::NewLogEntry(_)));
        }
    }

    #[tokio::test]
    async fn test_drain_respects_max_per_tick() {
        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);

        for i in 0..5 {
            let entry =
                parse_jsonl_line(&format!(r#"{{"type": "user", "sessionId": "s{}"}}"#, i)).unwrap();
            tx.send(WatcherEvent::NewEntry {
                source: PathBuf::from("/fake/s.jsonl"),
                entry: Box::new(entry),
            })
            .await
            .unwrap();
        }

        // Only drain 3
        let events = drain_log_entries(&mut rx, 3);
        assert_eq!(events.len(), 3);

        // Remaining 2 should still be in the channel
        let remaining = drain_log_entries(&mut rx, 100);
        assert_eq!(remaining.len(), 2);
    }

    #[tokio::test]
    async fn test_drain_converts_new_file_detected_and_skips_errors() {
        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);

        // Send a NewFileDetected event (should be converted)
        tx.send(WatcherEvent::NewFileDetected {
            path: PathBuf::from("/fake/new.jsonl"),
        })
        .await
        .unwrap();

        // Send an Error event (should be skipped)
        tx.send(WatcherEvent::Error("test error".to_string()))
            .await
            .unwrap();

        // Send a real entry
        let entry = parse_jsonl_line(r#"{"type": "user", "sessionId": "s1"}"#).unwrap();
        tx.send(WatcherEvent::NewEntry {
            source: PathBuf::from("/fake/s1.jsonl"),
            entry: Box::new(entry),
        })
        .await
        .unwrap();

        let events = drain_log_entries(&mut rx, 100);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], AppEvent::NewFileDetected(p) if p == &PathBuf::from("/fake/new.jsonl"))
        );
        assert!(matches!(&events[1], AppEvent::NewLogEntry(e) if e.entry_type == EntryType::User));
    }

    #[tokio::test]
    async fn test_drain_disconnected_channel() {
        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);
        drop(tx); // Disconnect the sender

        let events = drain_log_entries(&mut rx, 100);
        assert!(events.is_empty());
    }
}
