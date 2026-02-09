//! File watching and incremental JSONL reading for Claude Code sessions.
//!
//! This module provides filesystem-based monitoring of `.jsonl` log files
//! using the `notify` crate. It incrementally reads new lines from watched
//! files, parses them into `LogEntry` values, and sends them through a
//! tokio channel for downstream consumers.
//!
//! Key features:
//! - Per-file byte-offset tracking for efficient incremental reads
//! - Incomplete line buffering across multiple read events
//! - File truncation detection (resets offset when file shrinks)
//! - Recursive directory watching for subagent log files

use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Security constants
// ---------------------------------------------------------------------------

/// Maximum number of bytes to read in a single `read_new_entries` call.
/// Prevents OOM when a file grows very large between events (e.g. 64 MB).
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum size of the incomplete-line buffer (10 MB). If a single JSONL line
/// exceeds this size the buffer is discarded to prevent unbounded memory growth.
const MAX_INCOMPLETE_LINE_BUF: usize = 10 * 1024 * 1024;

use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::log_entry::{parse_jsonl_line, LogEntry};

// ---------------------------------------------------------------------------
// Per-file watch state
// ---------------------------------------------------------------------------

/// Tracks incremental reading state for a single watched file.
///
/// `byte_offset` records how far we have read, so the next read starts
/// where the previous one left off. `incomplete_line_buf` holds any
/// trailing bytes from the last read that did not end with a newline
/// (i.e. an incomplete JSONL line that will be completed on the next
/// write to the file).
pub(crate) struct FileWatchState {
    pub(crate) byte_offset: u64,
    incomplete_line_buf: String,
}

impl FileWatchState {
    pub(crate) fn new() -> Self {
        Self {
            byte_offset: 0,
            incomplete_line_buf: String::new(),
        }
    }

    /// Create a new `FileWatchState` starting at the given byte offset.
    ///
    /// This is useful when replay has already read the file up to a known
    /// position and the watcher should start tailing from that point.
    pub(crate) fn new_with_offset(offset: u64) -> Self {
        Self {
            byte_offset: offset,
            incomplete_line_buf: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// WatcherEvent
// ---------------------------------------------------------------------------

/// Events emitted by the watcher to downstream consumers.
#[derive(Debug)]
pub enum WatcherEvent {
    /// A new JSONL entry was successfully parsed from a watched file.
    NewEntry {
        #[allow(dead_code)]
        source: PathBuf,
        entry: Box<LogEntry>,
    },
    /// A new `.jsonl` file was detected (created) in the watched directory.
    NewFileDetected {
        #[allow(dead_code)]
        path: PathBuf,
    },
    /// An error occurred during watching or reading.
    Error(#[allow(dead_code)] String),
}

// ---------------------------------------------------------------------------
// WatcherError
// ---------------------------------------------------------------------------

/// Errors that can occur when setting up the file watcher.
#[derive(Debug)]
pub enum WatcherError {
    /// The underlying `notify` crate returned an error.
    Notify(notify::Error),
    /// The project directory does not exist or is not accessible.
    ProjectDirNotFound(PathBuf),
}

impl fmt::Display for WatcherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatcherError::Notify(e) => write!(f, "filesystem watcher error: {}", e),
            WatcherError::ProjectDirNotFound(p) => {
                write!(f, "project directory not found: {}", p.display())
            }
        }
    }
}

impl std::error::Error for WatcherError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WatcherError::Notify(e) => Some(e),
            WatcherError::ProjectDirNotFound(_) => None,
        }
    }
}

impl From<notify::Error> for WatcherError {
    fn from(err: notify::Error) -> Self {
        WatcherError::Notify(err)
    }
}

// ---------------------------------------------------------------------------
// WatcherHandle
// ---------------------------------------------------------------------------

/// Handle for cleanly shutting down the file watcher.
///
/// Signals the watcher loop to exit on the next timeout check and aborts the
/// tokio blocking task. Without this, the `spawn_blocking` thread blocks
/// indefinitely on `recv()` and prevents the tokio runtime from shutting down.
#[derive(Debug)]
pub struct WatcherHandle {
    shutdown: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl WatcherHandle {
    /// Signal the watcher to stop and abort the background task.
    pub fn shutdown(self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.handle.abort();
    }
}

// ---------------------------------------------------------------------------
// Helper: JSONL file filter
// ---------------------------------------------------------------------------

/// Returns `true` if the path has a `.jsonl` extension.
pub fn is_watched_jsonl(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

// ---------------------------------------------------------------------------
// Incremental reading
// ---------------------------------------------------------------------------

/// Read new entries from `path` starting at the byte offset recorded in `state`.
///
/// Returns a vector of successfully parsed `LogEntry` values. Lines that
/// fail to parse are silently skipped (with an optional verbose warning).
/// If the file has been truncated (its size is less than the recorded
/// offset), the offset is reset to 0 and the entire file is re-read.
///
/// Any trailing bytes that do not end with a newline are buffered in
/// `state.incomplete_line_buf` for the next call.
pub(crate) fn read_new_entries(
    path: &Path,
    state: &mut FileWatchState,
    verbose: bool,
) -> Vec<LogEntry> {
    let mut entries = Vec::new();

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if verbose {
                eprintln!("cc-tail: warning: could not open {}: {}", path.display(), e);
            }
            return entries;
        }
    };

    // Detect file truncation: if the file is smaller than our offset,
    // reset to the beginning.
    let file_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            if verbose {
                eprintln!("cc-tail: warning: could not stat {}: {}", path.display(), e);
            }
            return entries;
        }
    };

    if file_len < state.byte_offset {
        if verbose {
            eprintln!(
                "cc-tail: file truncated, resetting offset: {}",
                path.display()
            );
        }
        state.byte_offset = 0;
        state.incomplete_line_buf.clear();
    }

    // Nothing new to read
    if file_len == state.byte_offset {
        return entries;
    }

    // Seek to our last read position
    if let Err(e) = file.seek(SeekFrom::Start(state.byte_offset)) {
        if verbose {
            eprintln!(
                "cc-tail: warning: could not seek in {}: {}",
                path.display(),
                e
            );
        }
        return entries;
    }

    // Read new bytes, capped at MAX_READ_BYTES to prevent OOM
    let bytes_available = file_len - state.byte_offset;
    let read_limit = bytes_available.min(MAX_READ_BYTES);
    let mut buf = String::new();
    let bytes_read = match file.take(read_limit).read_to_string(&mut buf) {
        Ok(n) => n,
        Err(e) => {
            if verbose {
                eprintln!("cc-tail: warning: could not read {}: {}", path.display(), e);
            }
            return entries;
        }
    };

    state.byte_offset += bytes_read as u64;

    // Prepend any incomplete line from the previous read
    let full_text = if state.incomplete_line_buf.is_empty() {
        buf
    } else {
        let mut combined = std::mem::take(&mut state.incomplete_line_buf);
        combined.push_str(&buf);
        combined
    };

    // Split into lines; the last element may be incomplete if the
    // file write was partial (no trailing newline).
    let ends_with_newline = full_text.ends_with('\n');
    let mut lines: Vec<&str> = full_text.split('\n').collect();

    // If the text does not end with a newline, the last "line" is
    // incomplete and should be buffered for the next read.
    if !ends_with_newline {
        if let Some(last) = lines.pop() {
            if !last.is_empty() {
                if last.len() > MAX_INCOMPLETE_LINE_BUF {
                    // Discard oversized incomplete lines to prevent OOM
                    if verbose {
                        eprintln!(
                            "cc-tail: warning: discarding oversized incomplete line ({} bytes) in {}",
                            last.len(),
                            path.display()
                        );
                    }
                } else {
                    state.incomplete_line_buf = last.to_string();
                    // Check if buffer has grown too large after prepending
                    if state.incomplete_line_buf.len() > MAX_INCOMPLETE_LINE_BUF {
                        if verbose {
                            eprintln!(
                                "cc-tail: warning: incomplete line buffer exceeded {} bytes, resetting for {}",
                                MAX_INCOMPLETE_LINE_BUF,
                                path.display()
                            );
                        }
                        state.incomplete_line_buf.clear();
                    }
                }
            }
        }
    }

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_jsonl_line(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                if verbose {
                    eprintln!(
                        "cc-tail: warning: skipping malformed line in {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
    }

    entries
}

// ---------------------------------------------------------------------------
// Watcher entry point
// ---------------------------------------------------------------------------

/// Start watching a project directory for `.jsonl` file changes.
///
/// Sets up a `notify::RecommendedWatcher` in recursive mode and bridges
/// events from the synchronous `notify` channel to a tokio `mpsc` channel.
/// Returns a receiver for `WatcherEvent` values and a `JoinHandle` for
/// the background task.
///
/// # Arguments
///
/// * `project_dir` - Path to the Claude Code project `.claude/projects/...` directory.
/// * `verbose` - Whether to emit verbose diagnostic messages to stderr.
/// * `channel_capacity` - Capacity of the tokio mpsc channel.
/// * `initial_offsets` - Per-file byte offsets from replay. The watcher will
///   start reading each file from the given offset instead of byte 0.
///
/// # Errors
///
/// Returns `WatcherError::ProjectDirNotFound` if the directory does not exist,
/// or `WatcherError::Notify` if the watcher cannot be created.
pub fn start_watching(
    project_dir: PathBuf,
    verbose: bool,
    channel_capacity: usize,
    initial_offsets: HashMap<PathBuf, u64>,
) -> Result<(mpsc::Receiver<WatcherEvent>, WatcherHandle), WatcherError> {
    // Validate the project directory exists
    if !project_dir.is_dir() {
        return Err(WatcherError::ProjectDirNotFound(project_dir));
    }

    // Canonicalize the watched directory for consistent symlink comparison.
    let canonical_dir = project_dir
        .canonicalize()
        .map_err(|_| WatcherError::ProjectDirNotFound(project_dir.clone()))?;

    let (tx, rx) = mpsc::channel::<WatcherEvent>(channel_capacity);

    // Create the synchronous channel for notify
    let (notify_tx, notify_rx) = std::sync::mpsc::channel::<Result<Event, notify::Error>>();

    // Create the watcher
    let mut watcher = notify::RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            let _ = notify_tx.send(res);
        },
        notify::Config::default(),
    )?;

    // Start watching the directory recursively
    watcher.watch(project_dir.as_ref(), RecursiveMode::Recursive)?;

    // Shutdown flag checked by the watcher loop on each timeout.
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Spawn a blocking task to bridge notify events to the async world
    let handle = tokio::task::spawn_blocking(move || {
        // Keep the watcher alive for the lifetime of this task
        let _watcher = watcher;
        let mut file_states: HashMap<PathBuf, FileWatchState> = initial_offsets
            .into_iter()
            .map(|(path, offset)| (path, FileWatchState::new_with_offset(offset)))
            .collect();

        loop {
            match notify_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(Ok(event)) => {
                    process_notify_event(&event, &mut file_states, &tx, verbose, &canonical_dir);
                }
                Ok(Err(e)) => {
                    let _ = tx.blocking_send(WatcherEvent::Error(format!(
                        "filesystem watcher error: {}",
                        e
                    )));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if shutdown_clone.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // The notify sender was dropped; the watcher is shutting down.
                    break;
                }
            }
        }
    });

    Ok((rx, WatcherHandle { shutdown, handle }))
}

/// Validate that a path, after resolving symlinks, is still within the
/// watched directory. Returns `Some(canonical)` if valid, `None` otherwise.
fn validate_path_within_dir(path: &Path, watched_dir: &Path, verbose: bool) -> Option<PathBuf> {
    match path.canonicalize() {
        Ok(canonical) => {
            if canonical.starts_with(watched_dir) {
                Some(canonical)
            } else if verbose {
                eprintln!(
                    "cc-tail: warning: path {} resolves to {} which is outside watched directory {}",
                    path.display(),
                    canonical.display(),
                    watched_dir.display()
                );
                None
            } else {
                None
            }
        }
        Err(e) => {
            // File may have been deleted between the event and validation;
            // this is expected for Remove events, so only warn in verbose mode.
            if verbose {
                eprintln!(
                    "cc-tail: warning: could not canonicalize {}: {}",
                    path.display(),
                    e
                );
            }
            None
        }
    }
}

/// Process a single notify event, reading new entries and sending them
/// through the channel.
fn process_notify_event(
    event: &Event,
    file_states: &mut HashMap<PathBuf, FileWatchState>,
    tx: &mpsc::Sender<WatcherEvent>,
    verbose: bool,
    watched_dir: &Path,
) {
    for path in &event.paths {
        if !is_watched_jsonl(path) {
            continue;
        }

        match event.kind {
            EventKind::Create(_) => {
                // Validate path is within the watched directory (symlink check)
                let validated_path = match validate_path_within_dir(path, watched_dir, verbose) {
                    Some(p) => p,
                    None => continue,
                };

                // New file detected
                let _ = tx.blocking_send(WatcherEvent::NewFileDetected {
                    path: validated_path.clone(),
                });
                // Also try to read any content that was written at creation time
                // (handles race condition where data is written before the watcher
                // sees the Modify event).
                let state = file_states
                    .entry(validated_path.clone())
                    .or_insert_with(FileWatchState::new);
                let entries = read_new_entries(&validated_path, state, verbose);
                for entry in entries {
                    let _ = tx.blocking_send(WatcherEvent::NewEntry {
                        source: validated_path.clone(),
                        entry: Box::new(entry),
                    });
                }
            }
            EventKind::Modify(_) => {
                // Validate path is within the watched directory (symlink check)
                let validated_path = match validate_path_within_dir(path, watched_dir, verbose) {
                    Some(p) => p,
                    None => continue,
                };

                let state = file_states
                    .entry(validated_path.clone())
                    .or_insert_with(FileWatchState::new);
                let entries = read_new_entries(&validated_path, state, verbose);
                for entry in entries {
                    let _ = tx.blocking_send(WatcherEvent::NewEntry {
                        source: validated_path.clone(),
                        entry: Box::new(entry),
                    });
                }
            }
            EventKind::Remove(_) => {
                // Prune deleted files from file_states to prevent unbounded growth.
                // Try to resolve symlink first; if that fails (file already gone),
                // fall back to the original path for removal.
                let key = path.canonicalize().unwrap_or_else(|_| path.clone());
                file_states.remove(&key);
            }
            _ => {
                // Ignore Access, etc.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    // -- Helper: create a temp file with content and return path + state ------

    fn create_temp_jsonl(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    // -- 1. read_new_entries with complete lines ------------------------------

    #[test]
    fn test_read_new_entries_complete_lines() {
        let tmp = TempDir::new().unwrap();
        let content = concat!(
            r#"{"type": "user", "sessionId": "s1"}"#,
            "\n",
            r#"{"type": "assistant", "sessionId": "s1"}"#,
            "\n",
        );
        let path = create_temp_jsonl(tmp.path(), "test.jsonl", content);
        let mut state = FileWatchState::new();

        let entries = read_new_entries(&path, &mut state, false);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_type, crate::log_entry::EntryType::User);
        assert_eq!(
            entries[1].entry_type,
            crate::log_entry::EntryType::Assistant
        );
        // Offset should be at end of file
        assert_eq!(state.byte_offset, content.len() as u64);
        assert!(state.incomplete_line_buf.is_empty());
    }

    // -- 2. Incomplete line buffering across multiple reads --------------------

    #[test]
    fn test_incomplete_line_buffering() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("incremental.jsonl");

        // First write: one complete line + a partial second line
        let part1 = r#"{"type": "user", "sessionId": "s1"}
{"type": "assis"#;
        std::fs::write(&path, part1).unwrap();

        let mut state = FileWatchState::new();
        let entries1 = read_new_entries(&path, &mut state, false);

        assert_eq!(entries1.len(), 1);
        assert_eq!(entries1[0].entry_type, crate::log_entry::EntryType::User);
        // The incomplete line should be buffered
        assert_eq!(state.incomplete_line_buf, r#"{"type": "assis"#);

        // Second write: complete the partial line
        let part2 = concat!(
            r#"{"type": "user", "sessionId": "s1"}
{"type": "assis"#,
            r#"tant", "sessionId": "s1"}
"#,
        );
        std::fs::write(&path, part2).unwrap();

        let entries2 = read_new_entries(&path, &mut state, false);

        assert_eq!(entries2.len(), 1);
        assert_eq!(
            entries2[0].entry_type,
            crate::log_entry::EntryType::Assistant
        );
        assert!(state.incomplete_line_buf.is_empty());
    }

    // -- 3. Multi-event sequence ----------------------------------------------

    #[test]
    fn test_multi_event_sequence() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("multi.jsonl");

        // Write initial content
        let line1 = r#"{"type": "user", "sessionId": "s1"}"#;
        std::fs::write(&path, format!("{}\n", line1)).unwrap();

        let mut state = FileWatchState::new();

        // First read
        let entries1 = read_new_entries(&path, &mut state, false);
        assert_eq!(entries1.len(), 1);

        // Append more content
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let line2 = r#"{"type": "assistant", "sessionId": "s1"}"#;
        writeln!(file, "{}", line2).unwrap();
        drop(file);

        // Second read should only get the new line
        let entries2 = read_new_entries(&path, &mut state, false);
        assert_eq!(entries2.len(), 1);
        assert_eq!(
            entries2[0].entry_type,
            crate::log_entry::EntryType::Assistant
        );

        // Append yet more content
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let line3 = r#"{"type": "progress", "sessionId": "s1"}"#;
        let line4 = r#"{"type": "system", "sessionId": "s1"}"#;
        writeln!(file, "{}", line3).unwrap();
        writeln!(file, "{}", line4).unwrap();
        drop(file);

        // Third read should get both new lines
        let entries3 = read_new_entries(&path, &mut state, false);
        assert_eq!(entries3.len(), 2);
        assert_eq!(
            entries3[0].entry_type,
            crate::log_entry::EntryType::Progress
        );
        assert_eq!(entries3[1].entry_type, crate::log_entry::EntryType::System);
    }

    // -- 4. Malformed line handling (skip bad lines) --------------------------

    #[test]
    fn test_malformed_lines_are_skipped() {
        let tmp = TempDir::new().unwrap();
        let content = concat!(
            r#"{"type": "user", "sessionId": "s1"}"#,
            "\n",
            "this is not valid json\n",
            r#"{"type": "assistant", "sessionId": "s1"}"#,
            "\n",
            "{broken json\n",
        );
        let path = create_temp_jsonl(tmp.path(), "malformed.jsonl", content);
        let mut state = FileWatchState::new();

        let entries = read_new_entries(&path, &mut state, false);

        // Only the two valid entries should be returned
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_type, crate::log_entry::EntryType::User);
        assert_eq!(
            entries[1].entry_type,
            crate::log_entry::EntryType::Assistant
        );
    }

    // -- 5. File truncation detection -----------------------------------------

    #[test]
    fn test_file_truncation_detection() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("truncated.jsonl");

        // Write initial content
        let initial = concat!(
            r#"{"type": "user", "sessionId": "s1"}"#,
            "\n",
            r#"{"type": "assistant", "sessionId": "s1"}"#,
            "\n",
        );
        std::fs::write(&path, initial).unwrap();

        let mut state = FileWatchState::new();

        // First read: consume everything
        let entries1 = read_new_entries(&path, &mut state, false);
        assert_eq!(entries1.len(), 2);
        assert_eq!(state.byte_offset, initial.len() as u64);

        // Truncate the file and write shorter content
        let new_content = concat!(r#"{"type": "progress", "sessionId": "s2"}"#, "\n",);
        std::fs::write(&path, new_content).unwrap();

        // The file is now shorter than byte_offset, so truncation should be detected
        let entries2 = read_new_entries(&path, &mut state, true);
        assert_eq!(entries2.len(), 1);
        assert_eq!(
            entries2[0].entry_type,
            crate::log_entry::EntryType::Progress
        );
        assert_eq!(entries2[0].session_id.as_deref(), Some("s2"));
        assert_eq!(state.byte_offset, new_content.len() as u64);
    }

    // -- 6. is_watched_jsonl filtering ----------------------------------------

    #[test]
    fn test_is_watched_jsonl_true() {
        assert!(is_watched_jsonl(Path::new("session.jsonl")));
        assert!(is_watched_jsonl(Path::new("/some/path/agent-abc.jsonl")));
        assert!(is_watched_jsonl(Path::new("deep/nested/file.jsonl")));
    }

    #[test]
    fn test_is_watched_jsonl_false() {
        assert!(!is_watched_jsonl(Path::new("session.json")));
        assert!(!is_watched_jsonl(Path::new("readme.txt")));
        assert!(!is_watched_jsonl(Path::new("data.csv")));
        assert!(!is_watched_jsonl(Path::new("noextension")));
        assert!(!is_watched_jsonl(Path::new(".jsonl"))); // only extension, no stem
    }

    // -- 7. Empty file --------------------------------------------------------

    #[test]
    fn test_empty_file() {
        let tmp = TempDir::new().unwrap();
        let path = create_temp_jsonl(tmp.path(), "empty.jsonl", "");
        let mut state = FileWatchState::new();

        let entries = read_new_entries(&path, &mut state, false);
        assert!(entries.is_empty());
        assert_eq!(state.byte_offset, 0);
        assert!(state.incomplete_line_buf.is_empty());
    }

    // -- 8. FileWatchState::new initialization --------------------------------

    #[test]
    fn test_file_watch_state_new() {
        let state = FileWatchState::new();
        assert_eq!(state.byte_offset, 0);
        assert!(state.incomplete_line_buf.is_empty());
    }

    // -- 9. No re-read after consuming everything -----------------------------

    #[test]
    fn test_no_reread_when_no_new_content() {
        let tmp = TempDir::new().unwrap();
        let content = r#"{"type": "user", "sessionId": "s1"}
"#;
        let path = create_temp_jsonl(tmp.path(), "noreread.jsonl", content);
        let mut state = FileWatchState::new();

        // First read
        let entries1 = read_new_entries(&path, &mut state, false);
        assert_eq!(entries1.len(), 1);

        // Second read with no new content should return empty
        let entries2 = read_new_entries(&path, &mut state, false);
        assert!(entries2.is_empty());
    }

    // -- 10. WatcherError Display ---------------------------------------------

    #[test]
    fn test_watcher_error_display_notify() {
        let err = WatcherError::Notify(notify::Error::generic("test"));
        let msg = format!("{}", err);
        assert!(msg.contains("filesystem watcher error"));
    }

    #[test]
    fn test_watcher_error_display_project_dir_not_found() {
        let err = WatcherError::ProjectDirNotFound(PathBuf::from("/missing/dir"));
        let msg = format!("{}", err);
        assert!(msg.contains("project directory not found"));
        assert!(msg.contains("/missing/dir"));
    }

    // -- 11. Whitespace-only lines are skipped --------------------------------

    #[test]
    fn test_whitespace_only_lines_skipped() {
        let tmp = TempDir::new().unwrap();
        let content = concat!(
            r#"{"type": "user", "sessionId": "s1"}"#,
            "\n",
            "   \n",
            "\n",
            r#"{"type": "assistant", "sessionId": "s1"}"#,
            "\n",
        );
        let path = create_temp_jsonl(tmp.path(), "whitespace.jsonl", content);
        let mut state = FileWatchState::new();

        let entries = read_new_entries(&path, &mut state, false);
        assert_eq!(entries.len(), 2);
    }

    // -- 12. is_watched_jsonl with dotfile path (edge case) -------------------

    #[test]
    fn test_is_watched_jsonl_dotfile() {
        // A file named ".jsonl" has no stem, only extension
        // On most platforms, Path::extension() returns Some("jsonl") for ".jsonl"
        // but this is an edge case we should handle gracefully.
        // The behavior depends on the platform; we just verify no panic.
        let _ = is_watched_jsonl(Path::new(".jsonl"));
    }

    // -- 13. start_watching with nonexistent directory returns error ----------

    #[tokio::test]
    async fn test_start_watching_nonexistent_dir() {
        let result = start_watching(
            PathBuf::from("/nonexistent/path/12345"),
            false,
            16,
            HashMap::new(),
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            WatcherError::ProjectDirNotFound(p) => {
                assert_eq!(p, PathBuf::from("/nonexistent/path/12345"));
            }
            other => panic!("expected ProjectDirNotFound, got: {:?}", other),
        }
    }

    // -- 14. start_watching with valid directory succeeds ---------------------

    #[tokio::test]
    async fn test_start_watching_valid_dir() {
        let tmp = TempDir::new().unwrap();
        let result = start_watching(tmp.path().to_path_buf(), false, 16, HashMap::new());
        assert!(result.is_ok());

        let (_rx, handle) = result.unwrap();
        // Clean up: signal the watcher to shut down
        handle.shutdown();
    }

    // -- 15. process_notify_event ignores non-jsonl files --------------------

    #[test]
    fn test_process_notify_event_ignores_non_jsonl() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();

        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);
        let mut file_states = HashMap::new();

        let event = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![PathBuf::from("/some/file.txt")],
            attrs: Default::default(),
        };

        process_notify_event(&event, &mut file_states, &tx, false, &watched_dir);

        // No events should have been sent
        assert!(rx.try_recv().is_err());
    }

    // -- 16. process_notify_event sends NewFileDetected on Create ------------

    #[test]
    fn test_process_notify_event_create_sends_new_file_detected() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();
        let path = create_temp_jsonl(tmp.path(), "new.jsonl", "");
        let canonical_path = path.canonicalize().unwrap();

        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);
        let mut file_states = HashMap::new();

        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };

        process_notify_event(&event, &mut file_states, &tx, false, &watched_dir);

        // Should receive NewFileDetected (with canonical path)
        match rx.try_recv() {
            Ok(WatcherEvent::NewFileDetected { path: p }) => {
                assert_eq!(p, canonical_path);
            }
            other => panic!("expected NewFileDetected, got: {:?}", other),
        }
    }

    // -- 17. process_notify_event reads entries on Modify --------------------

    #[test]
    fn test_process_notify_event_modify_reads_entries() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();
        let content = r#"{"type": "user", "sessionId": "s1"}
"#;
        let path = create_temp_jsonl(tmp.path(), "modify.jsonl", content);
        let canonical_path = path.canonicalize().unwrap();

        let (tx, mut rx) = mpsc::channel::<WatcherEvent>(16);
        let mut file_states = HashMap::new();

        let event = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };

        process_notify_event(&event, &mut file_states, &tx, false, &watched_dir);

        // Should receive NewEntry (with canonical path)
        match rx.try_recv() {
            Ok(WatcherEvent::NewEntry { source, entry }) => {
                assert_eq!(source, canonical_path);
                assert_eq!(entry.entry_type, crate::log_entry::EntryType::User);
            }
            other => panic!("expected NewEntry, got: {:?}", other),
        }
    }

    // -- 18. Nonexistent file in read_new_entries returns empty ---------------

    #[test]
    fn test_read_new_entries_nonexistent_file() {
        let mut state = FileWatchState::new();
        let entries = read_new_entries(Path::new("/nonexistent/file.jsonl"), &mut state, false);
        assert!(entries.is_empty());
    }

    // ========================================================================
    // Security tests
    // ========================================================================

    // -- 19. MAX_READ_BYTES caps incremental read size -----------------------

    #[test]
    fn test_max_read_bytes_constant_is_reasonable() {
        // Verify the constant is set to 64 MB
        assert_eq!(MAX_READ_BYTES, 64 * 1024 * 1024);
    }

    // -- 20. Incomplete line buffer overflow is discarded --------------------

    #[test]
    fn test_incomplete_line_buf_overflow_discarded() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("overflow.jsonl");

        // Create a file with a very long incomplete line (exceeding
        // MAX_INCOMPLETE_LINE_BUF). We use a smaller threshold for testing
        // by writing content that would normally be buffered.
        // Since we can't easily write 10MB in a unit test, we test the
        // logic by manually setting state and verifying behavior.
        let mut state = FileWatchState::new();

        // Simulate an oversized incomplete_line_buf by setting it directly
        state.incomplete_line_buf = "x".repeat(MAX_INCOMPLETE_LINE_BUF + 1);
        // The buffer is already oversized, so the next read should detect
        // this in the prepend logic. Write a file with just a newline-terminated
        // line.
        let content = r#"{"type": "user", "sessionId": "s1"}
"#;
        std::fs::write(&path, content).unwrap();

        let entries = read_new_entries(&path, &mut state, false);

        // The oversized buffer gets prepended to the first line, making it
        // unparseable, but subsequent complete lines should still parse.
        // The incomplete_line_buf should be cleared after processing.
        assert!(state.incomplete_line_buf.is_empty());
        // The combined "xxx...x{\"type\": \"user\"...}" will fail to parse,
        // so we expect 0 entries from this read.
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_incomplete_line_buf_max_constant() {
        // Verify the constant is set to 10 MB
        assert_eq!(MAX_INCOMPLETE_LINE_BUF, 10 * 1024 * 1024);
    }

    // -- 21. Symlink validation rejects paths outside watched dir -----------

    #[test]
    fn test_validate_path_within_dir_rejects_outside() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let watched_dir = tmp1.path().canonicalize().unwrap();

        // Create a file in tmp2 (outside watched dir)
        let outside_file = create_temp_jsonl(tmp2.path(), "outside.jsonl", "");

        let result = validate_path_within_dir(&outside_file, &watched_dir, false);
        assert!(
            result.is_none(),
            "path outside watched dir should be rejected"
        );
    }

    #[test]
    fn test_validate_path_within_dir_accepts_inside() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();

        let inside_file = create_temp_jsonl(tmp.path(), "inside.jsonl", "");

        let result = validate_path_within_dir(&inside_file, &watched_dir, false);
        assert!(
            result.is_some(),
            "path inside watched dir should be accepted"
        );
    }

    #[test]
    fn test_validate_path_within_dir_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();

        let result =
            validate_path_within_dir(Path::new("/nonexistent/path.jsonl"), &watched_dir, false);
        assert!(result.is_none(), "nonexistent path should be rejected");
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_path_rejects_symlink_outside_dir() {
        let watched = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let watched_dir = watched.path().canonicalize().unwrap();

        // Create a real file outside the watched directory
        let real_file = create_temp_jsonl(outside.path(), "secret.jsonl", "secret data");

        // Create a symlink inside the watched directory pointing outside
        let symlink_path = watched.path().join("link.jsonl");
        std::os::unix::fs::symlink(&real_file, &symlink_path).unwrap();

        let result = validate_path_within_dir(&symlink_path, &watched_dir, false);
        assert!(
            result.is_none(),
            "symlink pointing outside watched dir should be rejected"
        );
    }

    // -- 22. Remove events prune file_states --------------------------------

    #[test]
    fn test_remove_event_prunes_file_states() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();
        let path = create_temp_jsonl(
            tmp.path(),
            "removable.jsonl",
            r#"{"type": "user", "sessionId": "s1"}
"#,
        );
        let canonical_path = path.canonicalize().unwrap();

        let (tx, _rx) = mpsc::channel::<WatcherEvent>(16);
        let mut file_states = HashMap::new();

        // First, simulate a Modify event to populate file_states
        let modify_event = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        process_notify_event(&modify_event, &mut file_states, &tx, false, &watched_dir);
        assert!(
            file_states.contains_key(&canonical_path),
            "file_states should contain the file after Modify event"
        );

        // Now simulate a Remove event
        let remove_event = Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        process_notify_event(&remove_event, &mut file_states, &tx, false, &watched_dir);

        // The file_states entry should be pruned.
        // Note: the file still exists on disk in the test (we didn't actually delete it),
        // but the Remove event handler should still remove it from the map.
        // Since the file still exists, canonicalize will succeed and produce the canonical path.
        assert!(
            !file_states.contains_key(&canonical_path),
            "file_states should be pruned after Remove event"
        );
    }

    // -- 23. Remove event with already-deleted file --------------------------

    #[test]
    fn test_remove_event_with_deleted_file() {
        let tmp = TempDir::new().unwrap();
        let watched_dir = tmp.path().canonicalize().unwrap();
        let path = tmp.path().join("deleted.jsonl");

        // Write and read the file to get it into file_states
        std::fs::write(
            &path,
            r#"{"type": "user", "sessionId": "s1"}
"#,
        )
        .unwrap();

        let (tx, _rx) = mpsc::channel::<WatcherEvent>(16);
        let mut file_states = HashMap::new();

        // Add entry to file_states using the raw path (simulating what happens
        // when canonicalize fails because file is already gone)
        file_states.insert(path.clone(), FileWatchState::new());

        // Now actually delete the file
        std::fs::remove_file(&path).unwrap();

        // Simulate Remove event -- canonicalize will fail, falls back to raw path
        let remove_event = Event {
            kind: EventKind::Remove(notify::event::RemoveKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };
        process_notify_event(&remove_event, &mut file_states, &tx, false, &watched_dir);

        // file_states should be pruned using the fallback raw path
        assert!(
            !file_states.contains_key(&path),
            "file_states should be pruned even when file is already deleted"
        );
    }
}
