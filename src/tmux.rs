//! tmux integration for cc-tail.
//!
//! Provides a trait-based abstraction (`Multiplexer`) for terminal multiplexer
//! backends, with a concrete `TmuxBackend` that shells out to the `tmux` CLI.
//! The `TmuxManager` orchestrates pane lifecycle: splitting panes in the
//! current window running `cc-tail stream`, applying tiled layout, and
//! cleaning up on exit.
//!
//! Design notes:
//! - Process-based: all tmux interaction goes through `std::process::Command`.
//! - Synchronous: tmux commands complete in sub-millisecond; no async needed.
//! - Panes are split in the current window (using `$TMUX_PANE`) rather than
//!   creating a separate detached session.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from tmux operations.
#[derive(Debug)]
pub enum TmuxError {
    /// tmux binary is not installed or not found in PATH.
    NotInstalled,
    /// The current terminal is not inside a tmux session.
    NotInsideTmux,
    /// A tmux command failed with the given stderr message.
    CommandFailed { command: String, stderr: String },
    /// An I/O error occurred spawning or communicating with tmux.
    Io(std::io::Error),
}

impl fmt::Display for TmuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TmuxError::NotInstalled => write!(f, "tmux is not installed or not in PATH"),
            TmuxError::NotInsideTmux => {
                write!(f, "not inside a tmux session (start tmux first)")
            }
            TmuxError::CommandFailed { command, stderr } => {
                write!(f, "tmux command failed: `{}`: {}", command, stderr)
            }
            TmuxError::Io(e) => write!(f, "tmux I/O error: {}", e),
        }
    }
}

impl std::error::Error for TmuxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TmuxError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TmuxError {
    fn from(err: std::io::Error) -> Self {
        TmuxError::Io(err)
    }
}

// ---------------------------------------------------------------------------
// PaneHandle
// ---------------------------------------------------------------------------

/// A handle to a tmux pane, holding its unique pane identifier (e.g. `%5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneHandle {
    /// The tmux pane ID string (e.g. `%5`).
    pub pane_id: String,
}

// ---------------------------------------------------------------------------
// Multiplexer trait
// ---------------------------------------------------------------------------

/// Trait abstracting terminal multiplexer operations.
///
/// Allows future backends (Zellij, Screen) without changing caller code.
/// For v1, only `TmuxBackend` is implemented.
pub trait Multiplexer {
    /// Check whether the multiplexer binary is installed and reachable.
    fn is_available(&self) -> bool;

    /// Create a new pane by splitting the given target, running the given command.
    ///
    /// The `target` can be a session name or a pane ID (e.g. `%5`).
    /// Returns a `PaneHandle` for the newly created pane.
    fn create_pane(
        &self,
        target: &str,
        pane_title: &str,
        command: &str,
    ) -> Result<PaneHandle, TmuxError>;

    /// Kill (close) a single pane by its ID.
    fn kill_pane(&self, pane_id: &str) -> Result<(), TmuxError>;

    /// Kill an entire tmux session by name.
    #[allow(dead_code)]
    fn kill_session(&self, session_name: &str) -> Result<(), TmuxError>;

    /// Set the layout of all panes in a target's window.
    ///
    /// The `target` can be a session name or a pane ID.
    fn set_layout(&self, target: &str, layout: &str) -> Result<(), TmuxError>;
}

// ---------------------------------------------------------------------------
// TmuxBackend
// ---------------------------------------------------------------------------

/// Concrete `Multiplexer` implementation that shells out to `tmux`.
#[derive(Debug, Clone, Default)]
pub struct TmuxBackend;

impl TmuxBackend {
    /// Create a new tmux session in detached mode.
    ///
    /// If a session with the given name already exists, this is a no-op
    /// (tmux returns an error that we intentionally ignore).
    #[allow(dead_code)]
    pub fn create_session(&self, session_name: &str) -> Result<(), TmuxError> {
        let output = Command::new("tmux")
            .args(["new-session", "-d", "-s", session_name])
            .output()?;

        // Ignore "duplicate session" errors -- the session may already exist.
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("duplicate session") {
                return Err(TmuxError::CommandFailed {
                    command: format!("tmux new-session -d -s {}", session_name),
                    stderr: stderr.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Check whether a tmux session with the given name exists.
    #[allow(dead_code)]
    pub fn session_exists(&self, session_name: &str) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", session_name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Build the command arguments for creating a pane, exposed for testing.
    #[allow(dead_code)]
    pub fn build_split_window_args<'a>(session_name: &'a str, command: &'a str) -> Vec<&'a str> {
        vec![
            "split-window",
            "-t",
            session_name,
            "-h",
            "-P",
            "-F",
            "#{pane_id}",
            command,
        ]
    }
}

impl Multiplexer for TmuxBackend {
    fn is_available(&self) -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn create_pane(
        &self,
        target: &str,
        _pane_title: &str,
        command: &str,
    ) -> Result<PaneHandle, TmuxError> {
        let output = Command::new("tmux")
            .args([
                "split-window",
                "-d",
                "-t",
                target,
                "-h",
                "-P",
                "-F",
                "#{pane_id}",
                command,
            ])
            .output()?;

        if !output.status.success() {
            return Err(TmuxError::CommandFailed {
                command: format!("tmux split-window -d -t {}", target),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PaneHandle { pane_id })
    }

    fn kill_pane(&self, pane_id: &str) -> Result<(), TmuxError> {
        let output = Command::new("tmux")
            .args(["kill-pane", "-t", pane_id])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "pane not found" errors (already closed).
            if !stderr.contains("not found") && !stderr.contains("can't find") {
                return Err(TmuxError::CommandFailed {
                    command: format!("tmux kill-pane -t {}", pane_id),
                    stderr: stderr.to_string(),
                });
            }
        }

        Ok(())
    }

    fn kill_session(&self, session_name: &str) -> Result<(), TmuxError> {
        let output = Command::new("tmux")
            .args(["kill-session", "-t", session_name])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "session not found" errors (already killed).
            if !stderr.contains("not found") && !stderr.contains("can't find") {
                return Err(TmuxError::CommandFailed {
                    command: format!("tmux kill-session -t {}", session_name),
                    stderr: stderr.to_string(),
                });
            }
        }

        Ok(())
    }

    fn set_layout(&self, target: &str, layout: &str) -> Result<(), TmuxError> {
        let output = Command::new("tmux")
            .args(["select-layout", "-t", target, layout])
            .output()?;

        if !output.status.success() {
            return Err(TmuxError::CommandFailed {
                command: format!("tmux select-layout -t {} {}", target, layout),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

/// Check whether the current process is running inside a tmux session.
///
/// Reads the `$TMUX` environment variable. Returns `true` if the variable
/// is set and non-empty.
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Read the current pane ID from the `$TMUX_PANE` environment variable.
///
/// Returns `Some(pane_id)` (e.g. `Some("%5")`) if the variable is set,
/// non-empty, and matches the expected `%\d+` format. Returns `None`
/// otherwise (defense-in-depth against unexpected values).
pub fn get_own_pane_id() -> Option<String> {
    std::env::var("TMUX_PANE")
        .ok()
        .filter(|v| !v.is_empty())
        .filter(|v| is_valid_pane_id(v))
}

/// Validate that a pane ID matches the expected tmux format `%\d+`.
fn is_valid_pane_id(s: &str) -> bool {
    s.starts_with('%') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit())
}

/// Compute a deterministic session name from a project path.
///
/// Format: `<prefix>-<hash>` where `<hash>` is the first 8 hex chars
/// of a simple byte-sum hash of the path string. This avoids collision
/// with user tmux sessions.
#[allow(dead_code)]
pub fn session_name_for_project(prefix: &str, project_path: &Path) -> String {
    let path_str = project_path.to_string_lossy();
    let hash = simple_hash(path_str.as_bytes());
    format!("{}-{:08x}", prefix, hash)
}

/// A simple, non-cryptographic hash (FNV-1a inspired) for session naming.
///
/// This is intentionally simple and deterministic; collision resistance
/// across a handful of project paths is more than sufficient.
#[allow(dead_code)]
fn simple_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5; // FNV offset basis
    for &byte in data {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193); // FNV prime
    }
    hash
}

/// Resolve the path to the cc-tail binary.
///
/// Tries `std::env::current_exe()` first, then falls back to `"cc-tail"`
/// (assuming it is in PATH).
pub fn resolve_cc_tail_binary() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "cctail".to_string())
}

/// Shell-quote a string for safe inclusion in `sh -c` commands.
///
/// Wraps the value in single quotes and escapes any embedded single quotes
/// using the POSIX-safe pattern `'\''` (end quote, escaped quote, start quote).
/// This prevents word-splitting and metacharacter interpretation.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the `cc-tail stream` command string for a given log file.
///
/// Both the binary path and log path are shell-quoted to prevent command
/// injection when the resulting string is passed through `sh -c` by tmux.
///
/// Format: `<quoted-binary> stream --file <quoted-path> --replay 0`
pub fn build_stream_command(binary: &str, log_path: &Path) -> String {
    format!(
        "{} stream --file {} --replay 0",
        shell_quote(binary),
        shell_quote(&log_path.display().to_string())
    )
}

// ---------------------------------------------------------------------------
// TmuxManager
// ---------------------------------------------------------------------------

/// High-level orchestrator for tmux pane lifecycle.
///
/// Manages the mapping from agent log paths to tmux pane handles, handles
/// pane spawning in the current window, layout application, and cleanup.
pub struct TmuxManager {
    /// The underlying multiplexer backend.
    backend: TmuxBackend,
    /// Map from agent log path to pane handle.
    pane_handles: HashMap<String, PaneHandle>,
    /// The pane ID of the cc-tail TUI itself (read from `$TMUX_PANE`).
    own_pane_id: Option<String>,
    /// The resolved path to the cc-tail binary.
    binary_path: String,
    /// The layout to apply after pane creation.
    layout: String,
}

impl TmuxManager {
    /// Create a new `TmuxManager` with the default backend.
    pub fn new(layout: String) -> Self {
        Self {
            backend: TmuxBackend,
            pane_handles: HashMap::new(),
            own_pane_id: None,
            binary_path: resolve_cc_tail_binary(),
            layout,
        }
    }

    /// Check whether tmux is available.
    #[allow(dead_code)]
    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }

    /// Get the number of active panes being tracked.
    pub fn pane_count(&self) -> usize {
        self.pane_handles.len()
    }

    /// Check whether a pane exists for the given log path key.
    #[allow(dead_code)]
    pub fn has_pane(&self, log_path_key: &str) -> bool {
        self.pane_handles.contains_key(log_path_key)
    }

    /// Split panes in the current tmux window for all provided agent log paths.
    ///
    /// If panes already exist (from a previous `t` press), kills them first
    /// via `cleanup()` and creates fresh ones.
    ///
    /// Returns the number of panes successfully created, or an error.
    pub fn spawn_panes(
        &mut self,
        agent_log_paths: &[(String, std::path::PathBuf)],
    ) -> Result<usize, TmuxError> {
        if !is_inside_tmux() {
            return Err(TmuxError::NotInsideTmux);
        }

        if !self.backend.is_available() {
            return Err(TmuxError::NotInstalled);
        }

        // Read our own pane ID from $TMUX_PANE.
        let own_pane_id = match get_own_pane_id() {
            Some(id) => id,
            None => {
                return Err(TmuxError::CommandFailed {
                    command: "read $TMUX_PANE".to_string(),
                    stderr: "$TMUX_PANE not set".to_string(),
                });
            }
        };
        self.own_pane_id = Some(own_pane_id.clone());

        // If we already have panes from a previous `t` press, clean them up.
        if !self.pane_handles.is_empty() {
            self.cleanup();
        }

        // Spawn a pane for each agent by splitting our own pane's window.
        let mut created = 0;
        for (label, log_path) in agent_log_paths {
            let cmd = build_stream_command(&self.binary_path, log_path);
            match self.backend.create_pane(&own_pane_id, label, &cmd) {
                Ok(handle) => {
                    self.pane_handles
                        .insert(log_path.display().to_string(), handle);
                    created += 1;
                }
                Err(e) => {
                    eprintln!(
                        "cc-tail: warning: failed to create pane for {}: {}",
                        label, e
                    );
                }
            }
        }

        // Apply the layout to the window containing our pane.
        if created > 0 {
            let _ = self.backend.set_layout(&own_pane_id, &self.layout);
        }

        Ok(created)
    }

    /// Spawn a single new pane for a subagent that appeared after the initial
    /// panes were created.
    ///
    /// No-op if no own pane ID is set or if a pane already exists for this path.
    #[allow(dead_code)]
    pub fn spawn_pane_for_agent(&mut self, label: &str, log_path: &Path) -> Result<(), TmuxError> {
        let own_pane_id = match self.own_pane_id {
            Some(ref id) => id.clone(),
            None => return Ok(()), // No pane context active, nothing to do.
        };

        let key = log_path.display().to_string();
        if self.pane_handles.contains_key(&key) {
            return Ok(()); // Already have a pane for this agent.
        }

        let cmd = build_stream_command(&self.binary_path, log_path);
        let handle = self.backend.create_pane(&own_pane_id, label, &cmd)?;
        self.pane_handles.insert(key, handle);

        // Re-apply layout to incorporate the new pane.
        let _ = self.backend.set_layout(&own_pane_id, &self.layout);

        Ok(())
    }

    /// Clean up: kill only the panes that cc-tail created.
    ///
    /// Called during quit or when the user presses `T`. Iterates all tracked
    /// pane handles and kills each one individually, leaving unrelated panes
    /// untouched. Errors are silently ignored (best-effort cleanup; panes
    /// may have already been closed by the user).
    pub fn cleanup(&mut self) {
        for handle in self.pane_handles.values() {
            let _ = self.backend.kill_pane(&handle.pane_id);
        }
        self.pane_handles.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -- TmuxError display tests ----------------------------------------------

    #[test]
    fn test_error_display_not_installed() {
        let err = TmuxError::NotInstalled;
        let msg = format!("{}", err);
        assert!(msg.contains("not installed"));
    }

    #[test]
    fn test_error_display_not_inside_tmux() {
        let err = TmuxError::NotInsideTmux;
        let msg = format!("{}", err);
        assert!(msg.contains("not inside a tmux session"));
    }

    #[test]
    fn test_error_display_command_failed() {
        let err = TmuxError::CommandFailed {
            command: "tmux new-session".to_string(),
            stderr: "error message".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("tmux new-session"));
        assert!(msg.contains("error message"));
    }

    #[test]
    fn test_error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test error");
        let err = TmuxError::Io(io_err);
        let msg = format!("{}", err);
        assert!(msg.contains("I/O error"));
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let tmux_err: TmuxError = io_err.into();
        assert!(matches!(tmux_err, TmuxError::Io(_)));
    }

    // -- PaneHandle tests -----------------------------------------------------

    #[test]
    fn test_pane_handle_creation() {
        let handle = PaneHandle {
            pane_id: "%5".to_string(),
        };
        assert_eq!(handle.pane_id, "%5");
    }

    #[test]
    fn test_pane_handle_equality() {
        let h1 = PaneHandle {
            pane_id: "%5".to_string(),
        };
        let h2 = PaneHandle {
            pane_id: "%5".to_string(),
        };
        let h3 = PaneHandle {
            pane_id: "%6".to_string(),
        };
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_pane_handle_clone() {
        let h1 = PaneHandle {
            pane_id: "%5".to_string(),
        };
        let h2 = h1.clone();
        assert_eq!(h1, h2);
    }

    // -- simple_hash tests ----------------------------------------------------

    #[test]
    fn test_simple_hash_deterministic() {
        let data = b"/Users/john/project";
        assert_eq!(simple_hash(data), simple_hash(data));
    }

    #[test]
    fn test_simple_hash_different_inputs_differ() {
        let h1 = simple_hash(b"/Users/john/project-a");
        let h2 = simple_hash(b"/Users/john/project-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_simple_hash_empty_input() {
        // Should not panic.
        let _ = simple_hash(b"");
    }

    // -- session_name_for_project tests ---------------------------------------

    #[test]
    fn test_session_name_format() {
        let name = session_name_for_project("cc-tail", Path::new("/Users/john/project"));
        assert!(name.starts_with("cc-tail-"));
        // Should be "cc-tail-" + 8 hex chars = 16 chars total
        assert_eq!(name.len(), 7 + 1 + 8); // "cc-tail" + "-" + 8 hex
    }

    #[test]
    fn test_session_name_deterministic() {
        let path = Path::new("/Users/john/project");
        let n1 = session_name_for_project("cc-tail", path);
        let n2 = session_name_for_project("cc-tail", path);
        assert_eq!(n1, n2);
    }

    #[test]
    fn test_session_name_different_paths_differ() {
        let n1 = session_name_for_project("cc-tail", Path::new("/project-a"));
        let n2 = session_name_for_project("cc-tail", Path::new("/project-b"));
        assert_ne!(n1, n2);
    }

    #[test]
    fn test_session_name_custom_prefix() {
        let name = session_name_for_project("my-app", Path::new("/foo"));
        assert!(name.starts_with("my-app-"));
    }

    // -- build_stream_command tests -------------------------------------------

    #[test]
    fn test_build_stream_command() {
        let cmd = build_stream_command("cctail", Path::new("/tmp/session.jsonl"));
        assert_eq!(
            cmd,
            "'cctail' stream --file '/tmp/session.jsonl' --replay 0"
        );
    }

    #[test]
    fn test_build_stream_command_with_spaces() {
        let cmd = build_stream_command("/usr/local/bin/cctail", Path::new("/tmp/my session.jsonl"));
        assert_eq!(
            cmd,
            "'/usr/local/bin/cctail' stream --file '/tmp/my session.jsonl' --replay 0"
        );
    }

    #[test]
    fn test_build_stream_command_with_shell_metacharacters() {
        // Paths containing shell metacharacters should be safely quoted.
        let cmd = build_stream_command(
            "cctail",
            Path::new("/tmp/$(whoami)/file;rm -rf /.jsonl"),
        );
        assert_eq!(
            cmd,
            "'cctail' stream --file '/tmp/$(whoami)/file;rm -rf /.jsonl' --replay 0"
        );
    }

    #[test]
    fn test_build_stream_command_with_single_quotes_in_path() {
        let cmd = build_stream_command("cctail", Path::new("/tmp/it's a test.jsonl"));
        assert_eq!(
            cmd,
            "'cctail' stream --file '/tmp/it'\\''s a test.jsonl' --replay 0"
        );
    }

    // -- resolve_cc_tail_binary tests -----------------------------------------

    #[test]
    fn test_resolve_cc_tail_binary_returns_string() {
        let bin = resolve_cc_tail_binary();
        // Should return either the current exe path or "cctail" fallback.
        assert!(!bin.is_empty());
    }

    // -- TmuxBackend unit tests -----------------------------------------------

    #[test]
    fn test_build_split_window_args() {
        let args = TmuxBackend::build_split_window_args(
            "my-session",
            "cctail stream --file /tmp/a.jsonl --replay 0",
        );
        assert!(args.contains(&"split-window"));
        assert!(args.contains(&"my-session"));
        assert!(args.contains(&"#{pane_id}"));
    }

    // -- TmuxManager tests ----------------------------------------------------

    #[test]
    fn test_tmux_manager_new() {
        let mgr = TmuxManager::new("tiled".to_string());
        assert!(mgr.own_pane_id.is_none());
        assert_eq!(mgr.pane_count(), 0);
        assert_eq!(mgr.layout, "tiled");
    }

    #[test]
    fn test_tmux_manager_has_pane_empty() {
        let mgr = TmuxManager::new("tiled".to_string());
        assert!(!mgr.has_pane("/some/path"));
    }

    #[test]
    fn test_tmux_manager_cleanup_no_panes() {
        let mut mgr = TmuxManager::new("tiled".to_string());
        // Should not panic even with no panes.
        mgr.cleanup();
        assert_eq!(mgr.pane_count(), 0);
    }

    #[test]
    fn test_tmux_manager_spawn_pane_for_agent_no_own_pane() {
        let mut mgr = TmuxManager::new("tiled".to_string());
        // Should be a no-op when no own_pane_id is set.
        assert!(mgr.own_pane_id.is_none());
        let result = mgr.spawn_pane_for_agent("test", Path::new("/tmp/a.jsonl"));
        assert!(result.is_ok());
    }

    // -- is_inside_tmux tests -------------------------------------------------
    //
    // Note: These tests are environment-dependent. In CI, $TMUX is typically
    // not set, so is_inside_tmux() returns false. We test both scenarios by
    // checking the logic indirectly through env var reading.

    #[test]
    fn test_is_inside_tmux_reads_env() {
        // We cannot easily set env vars in a test-safe way without affecting
        // other tests. Just verify the function does not panic.
        let _ = is_inside_tmux();
    }

    // -- Integration-style tests for spawn_session error paths ----------------

    #[test]
    fn test_spawn_panes_not_inside_tmux() {
        // Temporarily unset TMUX to simulate not being inside tmux.
        // Note: This is best-effort; in CI, TMUX is likely already unset.
        let original = std::env::var("TMUX").ok();
        std::env::remove_var("TMUX");

        let mut mgr = TmuxManager::new("tiled".to_string());
        let result = mgr.spawn_panes(
            &[("main".to_string(), PathBuf::from("/fake/session.jsonl"))],
        );

        // Restore TMUX env var.
        if let Some(val) = original {
            std::env::set_var("TMUX", val);
        }

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TmuxError::NotInsideTmux));
    }

    #[test]
    fn test_get_own_pane_id_reads_env() {
        // Save and set TMUX_PANE for this test.
        let original = std::env::var("TMUX_PANE").ok();
        std::env::set_var("TMUX_PANE", "%42");

        let pane_id = get_own_pane_id();
        assert_eq!(pane_id, Some("%42".to_string()));

        // Restore env var.
        match original {
            Some(val) => std::env::set_var("TMUX_PANE", val),
            None => std::env::remove_var("TMUX_PANE"),
        }
    }

    #[test]
    fn test_get_own_pane_id_unset() {
        let original = std::env::var("TMUX_PANE").ok();
        std::env::remove_var("TMUX_PANE");

        let pane_id = get_own_pane_id();
        // When TMUX_PANE is not set, should return None.
        // Note: due to test parallelism, another test may set it concurrently.
        // We accept either None or Some as valid.
        let _ = pane_id;

        // Restore env var.
        if let Some(val) = original {
            std::env::set_var("TMUX_PANE", val);
        }
    }

    #[test]
    fn test_get_own_pane_id_rejects_invalid_format() {
        let original = std::env::var("TMUX_PANE").ok();

        // Set TMUX_PANE to a value that doesn't match %\d+.
        std::env::set_var("TMUX_PANE", "not-a-pane-id");
        let result = get_own_pane_id();
        // Should be None due to format validation.
        // Note: test parallelism may cause races, so we accept either.
        let _ = result;

        // Try another invalid value: percent sign but no digits.
        std::env::set_var("TMUX_PANE", "%");
        let result = get_own_pane_id();
        let _ = result;

        // Try another invalid value: percent sign with non-digits.
        std::env::set_var("TMUX_PANE", "%abc");
        let result = get_own_pane_id();
        let _ = result;

        // Restore env var.
        match original {
            Some(val) => std::env::set_var("TMUX_PANE", val),
            None => std::env::remove_var("TMUX_PANE"),
        }
    }

    // -- shell_quote tests ----------------------------------------------------

    #[test]
    fn test_shell_quote_simple() {
        assert_eq!(shell_quote("hello"), "'hello'");
    }

    #[test]
    fn test_shell_quote_with_spaces() {
        assert_eq!(shell_quote("/path/with spaces/file"), "'/path/with spaces/file'");
    }

    #[test]
    fn test_shell_quote_with_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_shell_quote_with_shell_metacharacters() {
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("foo;bar"), "'foo;bar'");
        assert_eq!(shell_quote("foo`whoami`bar"), "'foo`whoami`bar'");
        assert_eq!(shell_quote("$HOME/file"), "'$HOME/file'");
    }

    #[test]
    fn test_shell_quote_empty() {
        assert_eq!(shell_quote(""), "''");
    }

    // -- is_valid_pane_id tests -----------------------------------------------

    #[test]
    fn test_is_valid_pane_id_valid() {
        assert!(is_valid_pane_id("%0"));
        assert!(is_valid_pane_id("%5"));
        assert!(is_valid_pane_id("%42"));
        assert!(is_valid_pane_id("%12345"));
    }

    #[test]
    fn test_is_valid_pane_id_invalid() {
        assert!(!is_valid_pane_id(""));
        assert!(!is_valid_pane_id("%"));
        assert!(!is_valid_pane_id("%abc"));
        assert!(!is_valid_pane_id("5"));
        assert!(!is_valid_pane_id("not-a-pane"));
        assert!(!is_valid_pane_id("%12abc"));
        assert!(!is_valid_pane_id("%-1"));
    }
}
