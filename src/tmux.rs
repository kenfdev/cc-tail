//! tmux integration for cc-tail.
//!
//! Provides a trait-based abstraction (`Multiplexer`) for terminal multiplexer
//! backends, with a concrete `TmuxBackend` that shells out to the `tmux` CLI.
//! The `TmuxManager` orchestrates pane lifecycle: creating a session, spawning
//! per-agent panes running `cc-tail stream`, applying tiled layout, and
//! cleaning up on exit.
//!
//! Design notes:
//! - Process-based: all tmux interaction goes through `std::process::Command`.
//! - Synchronous: tmux commands complete in sub-millisecond; no async needed.
//! - Session naming: `<prefix>-<hash>` where hash is the first 8 hex chars of
//!   a simple hash of the project path, avoiding collision with user sessions.

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

    /// Create a new pane in the given session, running the given command.
    ///
    /// Returns a `PaneHandle` for the newly created pane.
    fn create_pane(
        &self,
        session_name: &str,
        pane_title: &str,
        command: &str,
    ) -> Result<PaneHandle, TmuxError>;

    /// Kill (close) a single pane by its ID.
    fn kill_pane(&self, pane_id: &str) -> Result<(), TmuxError>;

    /// Kill an entire tmux session by name.
    fn kill_session(&self, session_name: &str) -> Result<(), TmuxError>;

    /// Set the layout of all panes in a session's first window.
    fn set_layout(&self, session_name: &str, layout: &str) -> Result<(), TmuxError>;
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
    pub fn session_exists(&self, session_name: &str) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", session_name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Build the command arguments for creating a pane, exposed for testing.
    pub fn build_split_window_args<'a>(
        session_name: &'a str,
        command: &'a str,
    ) -> Vec<&'a str> {
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
        session_name: &str,
        _pane_title: &str,
        command: &str,
    ) -> Result<PaneHandle, TmuxError> {
        let output = Command::new("tmux")
            .args([
                "split-window",
                "-t",
                session_name,
                "-h",
                "-P",
                "-F",
                "#{pane_id}",
                command,
            ])
            .output()?;

        if !output.status.success() {
            return Err(TmuxError::CommandFailed {
                command: format!("tmux split-window -t {}", session_name),
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

    fn set_layout(&self, session_name: &str, layout: &str) -> Result<(), TmuxError> {
        let output = Command::new("tmux")
            .args(["select-layout", "-t", session_name, layout])
            .output()?;

        if !output.status.success() {
            return Err(TmuxError::CommandFailed {
                command: format!("tmux select-layout -t {} {}", session_name, layout),
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

/// Compute a deterministic session name from a project path.
///
/// Format: `<prefix>-<hash>` where `<hash>` is the first 8 hex chars
/// of a simple byte-sum hash of the path string. This avoids collision
/// with user tmux sessions.
pub fn session_name_for_project(prefix: &str, project_path: &Path) -> String {
    let path_str = project_path.to_string_lossy();
    let hash = simple_hash(path_str.as_bytes());
    format!("{}-{:08x}", prefix, hash)
}

/// A simple, non-cryptographic hash (FNV-1a inspired) for session naming.
///
/// This is intentionally simple and deterministic; collision resistance
/// across a handful of project paths is more than sufficient.
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
        .unwrap_or_else(|| "cc-tail".to_string())
}

/// Build the `cc-tail stream` command string for a given log file.
///
/// Format: `<binary> stream --file <path> --replay 0`
pub fn build_stream_command(binary: &str, log_path: &Path) -> String {
    format!(
        "{} stream --file {} --replay 0",
        binary,
        log_path.display()
    )
}

// ---------------------------------------------------------------------------
// TmuxManager
// ---------------------------------------------------------------------------

/// High-level orchestrator for tmux pane lifecycle.
///
/// Manages the mapping from agent log paths to tmux pane handles, handles
/// session creation, pane spawning, layout application, and cleanup.
pub struct TmuxManager {
    /// The underlying multiplexer backend.
    backend: TmuxBackend,
    /// Map from agent log path to pane handle.
    pane_handles: HashMap<String, PaneHandle>,
    /// The tmux session name (e.g. `cc-tail-a1b2c3d4`).
    session_name: Option<String>,
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
            session_name: None,
            binary_path: resolve_cc_tail_binary(),
            layout,
        }
    }

    /// Check whether tmux is available.
    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }

    /// Get the current session name, if any.
    pub fn session_name(&self) -> Option<&str> {
        self.session_name.as_deref()
    }

    /// Get the number of active panes being tracked.
    pub fn pane_count(&self) -> usize {
        self.pane_handles.len()
    }

    /// Check whether a pane exists for the given log path key.
    pub fn has_pane(&self, log_path_key: &str) -> bool {
        self.pane_handles.contains_key(log_path_key)
    }

    /// Set up a tmux session and spawn panes for all provided agent log paths.
    ///
    /// If a session already exists (from a previous `t` press), kills it first
    /// and creates a fresh one.
    ///
    /// Returns the number of panes successfully created, or an error.
    pub fn spawn_session(
        &mut self,
        session_prefix: &str,
        project_path: &Path,
        agent_log_paths: &[(String, std::path::PathBuf)],
    ) -> Result<usize, TmuxError> {
        if !is_inside_tmux() {
            return Err(TmuxError::NotInsideTmux);
        }

        if !self.backend.is_available() {
            return Err(TmuxError::NotInstalled);
        }

        let session_name = session_name_for_project(session_prefix, project_path);

        // If we already have a session, kill it first.
        if let Some(ref old_name) = self.session_name {
            let _ = self.backend.kill_session(old_name);
        }
        self.pane_handles.clear();

        // Create the new session.
        self.backend.create_session(&session_name)?;
        self.session_name = Some(session_name.clone());

        // Spawn a pane for each agent.
        let mut created = 0;
        for (label, log_path) in agent_log_paths {
            let cmd = build_stream_command(&self.binary_path, log_path);
            match self.backend.create_pane(&session_name, label, &cmd) {
                Ok(handle) => {
                    self.pane_handles.insert(log_path.display().to_string(), handle);
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

        // Apply the layout.
        if created > 0 {
            let _ = self.backend.set_layout(&session_name, &self.layout);
        }

        Ok(created)
    }

    /// Spawn a single new pane for a subagent that appeared after the initial
    /// session was created.
    ///
    /// No-op if no session exists or if a pane already exists for this path.
    pub fn spawn_pane_for_agent(
        &mut self,
        label: &str,
        log_path: &Path,
    ) -> Result<(), TmuxError> {
        let session_name = match self.session_name {
            Some(ref name) => name.clone(),
            None => return Ok(()), // No session active, nothing to do.
        };

        let key = log_path.display().to_string();
        if self.pane_handles.contains_key(&key) {
            return Ok(()); // Already have a pane for this agent.
        }

        let cmd = build_stream_command(&self.binary_path, log_path);
        let handle = self.backend.create_pane(&session_name, label, &cmd)?;
        self.pane_handles.insert(key, handle);

        // Re-apply layout to incorporate the new pane.
        let _ = self.backend.set_layout(&session_name, &self.layout);

        Ok(())
    }

    /// Clean up: kill the tmux session and all tracked panes.
    ///
    /// Called during quit. Errors are silently ignored (best-effort cleanup).
    pub fn cleanup(&mut self) {
        if let Some(ref session_name) = self.session_name {
            let _ = self.backend.kill_session(session_name);
        }
        self.pane_handles.clear();
        self.session_name = None;
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
        let cmd = build_stream_command("cc-tail", Path::new("/tmp/session.jsonl"));
        assert_eq!(cmd, "cc-tail stream --file /tmp/session.jsonl --replay 0");
    }

    #[test]
    fn test_build_stream_command_with_spaces() {
        let cmd = build_stream_command(
            "/usr/local/bin/cc-tail",
            Path::new("/tmp/my session.jsonl"),
        );
        assert_eq!(
            cmd,
            "/usr/local/bin/cc-tail stream --file /tmp/my session.jsonl --replay 0"
        );
    }

    // -- resolve_cc_tail_binary tests -----------------------------------------

    #[test]
    fn test_resolve_cc_tail_binary_returns_string() {
        let bin = resolve_cc_tail_binary();
        // Should return either the current exe path or "cc-tail" fallback.
        assert!(!bin.is_empty());
    }

    // -- TmuxBackend unit tests -----------------------------------------------

    #[test]
    fn test_build_split_window_args() {
        let args = TmuxBackend::build_split_window_args("my-session", "cc-tail stream --file /tmp/a.jsonl --replay 0");
        assert!(args.contains(&"split-window"));
        assert!(args.contains(&"my-session"));
        assert!(args.contains(&"#{pane_id}"));
    }

    // -- TmuxManager tests ----------------------------------------------------

    #[test]
    fn test_tmux_manager_new() {
        let mgr = TmuxManager::new("tiled".to_string());
        assert!(mgr.session_name().is_none());
        assert_eq!(mgr.pane_count(), 0);
        assert_eq!(mgr.layout, "tiled");
    }

    #[test]
    fn test_tmux_manager_has_pane_empty() {
        let mgr = TmuxManager::new("tiled".to_string());
        assert!(!mgr.has_pane("/some/path"));
    }

    #[test]
    fn test_tmux_manager_cleanup_no_session() {
        let mut mgr = TmuxManager::new("tiled".to_string());
        // Should not panic even with no session.
        mgr.cleanup();
        assert!(mgr.session_name().is_none());
        assert_eq!(mgr.pane_count(), 0);
    }

    #[test]
    fn test_tmux_manager_spawn_pane_for_agent_no_session() {
        let mut mgr = TmuxManager::new("tiled".to_string());
        // Should be a no-op when no session is active.
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
    fn test_spawn_session_not_inside_tmux() {
        // Temporarily unset TMUX to simulate not being inside tmux.
        // Note: This is best-effort; in CI, TMUX is likely already unset.
        let original = std::env::var("TMUX").ok();
        std::env::remove_var("TMUX");

        let mut mgr = TmuxManager::new("tiled".to_string());
        let result = mgr.spawn_session(
            "cc-tail",
            Path::new("/fake/project"),
            &[("main".to_string(), PathBuf::from("/fake/session.jsonl"))],
        );

        // Restore TMUX env var.
        if let Some(val) = original {
            std::env::set_var("TMUX", val);
        }

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TmuxError::NotInsideTmux));
    }
}
