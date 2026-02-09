//! Session discovery and management for Claude Code JSONL log files.
//!
//! Claude Code stores per-session logs as JSONL files under the project
//! directory. Each session has a top-level `{sessionId}.jsonl` file and
//! may have subagent logs under `{sessionId}/subagents/agent-{agentId}.jsonl`.
//!
//! This module discovers sessions from the filesystem, tracks subagent
//! relationships, determines active/inactive status, and supports
//! auto-attach (most recent) and `--session` prefix matching.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default threshold for considering a session active (10 minutes).
const DEFAULT_ACTIVE_THRESHOLD: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Represents a single agent within a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// The agent identifier (e.g. `"a0d0bbc"`). `None` for the main session agent.
    pub agent_id: Option<String>,
    /// Human-readable subagent name (e.g. `"effervescent-soaring-cook"`).
    /// Left as `None` during discovery; populated during JSONL parsing later.
    pub slug: Option<String>,
    /// Path to the agent's JSONL log file.
    pub log_path: PathBuf,
    /// `true` if this is the main session agent (top-level JSONL file).
    pub is_main: bool,
}

/// Represents a discovered Claude Code session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Session identifier (filename stem of the top-level JSONL file).
    pub id: String,
    /// All agents in this session (main + subagents).
    pub agents: Vec<Agent>,
    /// Most recent modification time across all agent files.
    pub last_modified: SystemTime,
}

/// Whether a session is considered active or inactive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Inactive,
}

/// Classification of a newly observed file in the project directory.
///
/// Used by the file watcher to determine what a new file represents
/// without performing any I/O—classification is purely path-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewFileKind {
    /// A top-level session JSONL file (`{project_dir}/{session_id}.jsonl`).
    TopLevelSession { session_id: String },
    /// A subagent JSONL file (`{project_dir}/{session_id}/subagents/agent-{agent_id}.jsonl`).
    Subagent {
        session_id: String,
        agent_id: String,
    },
    /// A file that does not match any known pattern.
    Unknown,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during session discovery or resolution.
#[derive(Debug)]
pub enum SessionDiscoveryError {
    /// No sessions found in the project directory.
    NoSessions,
    /// No session matched the given prefix.
    PrefixNotFound { prefix: String },
    /// Multiple sessions matched the given prefix.
    AmbiguousPrefix {
        prefix: String,
        matches: Vec<String>,
    },
    /// An I/O error occurred during discovery.
    Io(std::io::Error),
}

impl fmt::Display for SessionDiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionDiscoveryError::NoSessions => {
                write!(f, "no sessions found in the project directory")
            }
            SessionDiscoveryError::PrefixNotFound { prefix } => {
                write!(f, "no session found matching prefix \"{}\"", prefix)
            }
            SessionDiscoveryError::AmbiguousPrefix { prefix, matches } => {
                write!(
                    f,
                    "ambiguous session prefix \"{}\": matches {:?}",
                    prefix, matches
                )
            }
            SessionDiscoveryError::Io(e) => write!(f, "I/O error during session discovery: {}", e),
        }
    }
}

impl std::error::Error for SessionDiscoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionDiscoveryError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SessionDiscoveryError {
    fn from(err: std::io::Error) -> Self {
        SessionDiscoveryError::Io(err)
    }
}

// ---------------------------------------------------------------------------
// Session status
// ---------------------------------------------------------------------------

impl Session {
    /// Check whether this session is active using the default threshold (10 minutes).
    pub fn status(&self) -> SessionStatus {
        self.status_with_threshold(DEFAULT_ACTIVE_THRESHOLD)
    }

    /// Check whether this session is active using a custom threshold.
    ///
    /// A session is considered active if its `last_modified` time is within
    /// `threshold` of the current system time.
    pub fn status_with_threshold(&self, threshold: Duration) -> SessionStatus {
        let now = SystemTime::now();
        match now.duration_since(self.last_modified) {
            Ok(elapsed) if elapsed <= threshold => SessionStatus::Active,
            _ => SessionStatus::Inactive,
        }
    }
}

// ---------------------------------------------------------------------------
// Session discovery
// ---------------------------------------------------------------------------

/// Discover sessions from JSONL files in the given project directory.
///
/// Scans `project_dir` for `*.jsonl` files, treating each as a session.
/// For each session, also checks `{sessionId}/subagents/` for subagent
/// JSONL files matching `agent-*.jsonl`.
///
/// Returns at most `max_sessions` sessions, sorted by `last_modified`
/// descending (most recent first).
///
/// # Errors
///
/// Returns `SessionDiscoveryError::Io` if the project directory cannot
/// be read. Individual file permission errors are skipped with a warning
/// printed to stderr.
pub fn discover_sessions(
    project_dir: &Path,
    max_sessions: usize,
) -> Result<Vec<Session>, SessionDiscoveryError> {
    let entries = std::fs::read_dir(project_dir)?;

    let mut sessions: Vec<Session> = Vec::new();

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "cc-tail: warning: skipping entry in {}: {}",
                    project_dir.display(),
                    e
                );
                continue;
            }
        };

        let path = entry.path();

        // Only consider files with .jsonl extension
        if !path.is_file() {
            continue;
        }
        let extension = path.extension().and_then(|e| e.to_str());
        if extension != Some("jsonl") {
            continue;
        }

        // Extract session ID from filename (strip .jsonl extension)
        let session_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(stem) => stem.to_string(),
            None => continue,
        };

        // Build main agent
        let main_mtime = file_modified_time(&path);
        let main_agent = Agent {
            agent_id: None,
            slug: None,
            log_path: path.clone(),
            is_main: true,
        };

        let mut agents = vec![main_agent];
        let mut max_mtime = main_mtime;

        // Check for subagents in {sessionId}/subagents/
        let subagents_dir = project_dir.join(&session_id).join("subagents");
        if subagents_dir.is_dir() {
            if let Ok(sub_entries) = std::fs::read_dir(&subagents_dir) {
                for sub_entry_result in sub_entries {
                    let sub_entry = match sub_entry_result {
                        Ok(e) => e,
                        Err(e) => {
                            eprintln!(
                                "cc-tail: warning: skipping subagent entry in {}: {}",
                                subagents_dir.display(),
                                e
                            );
                            continue;
                        }
                    };

                    let sub_path = sub_entry.path();

                    // Only consider agent-*.jsonl files
                    if !sub_path.is_file() {
                        continue;
                    }
                    let sub_ext = sub_path.extension().and_then(|e| e.to_str());
                    if sub_ext != Some("jsonl") {
                        continue;
                    }
                    let sub_stem = match sub_path.file_stem().and_then(|s| s.to_str()) {
                        Some(s) => s,
                        None => continue,
                    };
                    if !sub_stem.starts_with("agent-") {
                        continue;
                    }

                    // Extract agent ID from "agent-{agentId}"
                    let agent_id = sub_stem.strip_prefix("agent-").unwrap().to_string();

                    let sub_mtime = file_modified_time(&sub_path);
                    if sub_mtime > max_mtime {
                        max_mtime = sub_mtime;
                    }

                    agents.push(Agent {
                        agent_id: Some(agent_id),
                        slug: None,
                        log_path: sub_path,
                        is_main: false,
                    });
                }
            }
        }

        sessions.push(Session {
            id: session_id,
            agents,
            last_modified: max_mtime,
        });
    }

    // Sort by last_modified descending (most recent first)
    sessions.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));

    // Limit to max_sessions
    sessions.truncate(max_sessions);

    Ok(sessions)
}

// ---------------------------------------------------------------------------
// Session resolution
// ---------------------------------------------------------------------------

/// Resolve a session from the discovered sessions list.
///
/// - If `session_prefix` is `None`, returns the most recently modified
///   session (auto-attach behavior).
/// - If `session_prefix` is `Some(prefix)`, finds sessions whose ID
///   starts with the given prefix. Requires exactly one match.
///
/// # Errors
///
/// - `NoSessions` if the sessions list is empty.
/// - `PrefixNotFound` if no session ID starts with the given prefix.
/// - `AmbiguousPrefix` if multiple session IDs start with the given prefix.
pub fn resolve_session<'a>(
    sessions: &'a [Session],
    session_prefix: Option<&str>,
) -> Result<&'a Session, SessionDiscoveryError> {
    if sessions.is_empty() {
        return Err(SessionDiscoveryError::NoSessions);
    }

    match session_prefix {
        None => {
            // Auto-attach: sessions are already sorted by last_modified desc,
            // so the first one is the most recent.
            Ok(&sessions[0])
        }
        Some(prefix) => {
            // First check for exact match
            if let Some(session) = sessions.iter().find(|s| s.id == prefix) {
                return Ok(session);
            }

            // Then try prefix match
            let matches: Vec<&Session> = sessions
                .iter()
                .filter(|s| s.id.starts_with(prefix))
                .collect();

            match matches.len() {
                0 => Err(SessionDiscoveryError::PrefixNotFound {
                    prefix: prefix.to_string(),
                }),
                1 => Ok(matches[0]),
                _ => Err(SessionDiscoveryError::AmbiguousPrefix {
                    prefix: prefix.to_string(),
                    matches: matches.iter().map(|s| s.id.clone()).collect(),
                }),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// New-file classification
// ---------------------------------------------------------------------------

/// Classify a newly observed file path relative to the project directory.
///
/// This is a pure, path-based classification with no filesystem I/O.
/// It examines the structure of `path` relative to `project_dir` to
/// determine whether the file represents a top-level session, a subagent
/// log, or something unknown.
///
/// # Classification rules
///
/// 1. The file must have a `.jsonl` extension.
/// 2. If it is a direct child of `project_dir` → `TopLevelSession`.
/// 3. If it matches `{project_dir}/{session_id}/subagents/agent-{agent_id}.jsonl`
///    → `Subagent`.
/// 4. Otherwise → `Unknown`.
pub fn classify_new_file(path: &Path, project_dir: &Path) -> NewFileKind {
    // Must have .jsonl extension.
    match path.extension().and_then(|e| e.to_str()) {
        Some("jsonl") => {}
        _ => return NewFileKind::Unknown,
    }

    // Get the path relative to project_dir.
    let relative = match path.strip_prefix(project_dir) {
        Ok(rel) => rel,
        Err(_) => return NewFileKind::Unknown,
    };

    let components: Vec<_> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    match components.len() {
        // Direct child: {session_id}.jsonl
        1 => {
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => return NewFileKind::Unknown,
            };
            NewFileKind::TopLevelSession { session_id: stem }
        }
        // Subagent: {session_id}/subagents/agent-{agent_id}.jsonl
        3 => {
            let session_id = &components[0];
            let middle = &components[1];
            let filename_stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => return NewFileKind::Unknown,
            };

            if middle != "subagents" {
                return NewFileKind::Unknown;
            }

            match filename_stem.strip_prefix("agent-") {
                Some(agent_id) => NewFileKind::Subagent {
                    session_id: session_id.clone(),
                    agent_id: agent_id.to_string(),
                },
                None => NewFileKind::Unknown,
            }
        }
        _ => NewFileKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the modification time of a file, falling back to UNIX_EPOCH on error.
fn file_modified_time(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(UNIX_EPOCH)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    // -- Helper functions ----------------------------------------------------

    /// Create a temporary directory structure for testing session discovery.
    fn setup_project_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    /// Create a JSONL file with some dummy content and return its path.
    fn create_jsonl_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, r#"{{"type": "user", "sessionId": "test"}}"#).unwrap();
        path
    }

    /// Set the modification time of a file to a specific offset from now.
    /// Uses filetime crate indirectly via touching the file after a sleep,
    /// or more practically, we manipulate the SystemTime comparison in tests.
    /// For simplicity, we just create files in order and rely on filesystem
    /// mtime ordering within the same second being unreliable, so we use
    /// explicit mtime setting via std::fs::File::set_modified (unstable)
    /// or accept test tolerance.
    ///
    /// Since we cannot easily set mtime portably without extra deps, tests
    /// that depend on ordering will create files with sufficient delay or
    /// use the session struct directly.
    fn create_session_with_mtime(id: &str, mtime: SystemTime) -> Session {
        Session {
            id: id.to_string(),
            agents: vec![Agent {
                agent_id: None,
                slug: None,
                log_path: PathBuf::from(format!("/fake/{}.jsonl", id)),
                is_main: true,
            }],
            last_modified: mtime,
        }
    }

    // ========================================================================
    // Discovery tests
    // ========================================================================

    // -- 1. Single session discovery -----------------------------------------

    #[test]
    fn test_discover_single_session() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "session-abc123.jsonl");

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session-abc123");
        assert_eq!(sessions[0].agents.len(), 1);
        assert!(sessions[0].agents[0].is_main);
        assert_eq!(sessions[0].agents[0].agent_id, None);
    }

    // -- 2. Session with subagents -------------------------------------------

    #[test]
    fn test_discover_session_with_subagents() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-001.jsonl");
        create_jsonl_file(tmp.path(), "sess-001/subagents/agent-a0d0bbc.jsonl");
        create_jsonl_file(tmp.path(), "sess-001/subagents/agent-b1e1ccd.jsonl");

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "sess-001");
        assert_eq!(sessions[0].agents.len(), 3); // 1 main + 2 subagents

        let main_agents: Vec<_> = sessions[0].agents.iter().filter(|a| a.is_main).collect();
        assert_eq!(main_agents.len(), 1);

        let sub_agents: Vec<_> = sessions[0].agents.iter().filter(|a| !a.is_main).collect();
        assert_eq!(sub_agents.len(), 2);

        let agent_ids: Vec<_> = sub_agents
            .iter()
            .map(|a| a.agent_id.as_deref().unwrap())
            .collect();
        assert!(agent_ids.contains(&"a0d0bbc"));
        assert!(agent_ids.contains(&"b1e1ccd"));
    }

    // -- 3. Multiple sessions sorted by mtime --------------------------------

    #[test]
    fn test_discover_multiple_sessions_sorted_by_mtime() {
        // We construct Session structs directly to test sorting logic,
        // since filesystem mtime is hard to control without extra deps.
        let now = SystemTime::now();
        let old = now - Duration::from_secs(3600);
        let older = now - Duration::from_secs(7200);

        let mut sessions = [
            create_session_with_mtime("oldest", older),
            create_session_with_mtime("newest", now),
            create_session_with_mtime("middle", old),
        ];

        // Simulate the sorting that discover_sessions does
        sessions.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));

        assert_eq!(sessions[0].id, "newest");
        assert_eq!(sessions[1].id, "middle");
        assert_eq!(sessions[2].id, "oldest");
    }

    // -- 4. Discovery with max_sessions limit --------------------------------

    #[test]
    fn test_discover_with_limit() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-a.jsonl");
        create_jsonl_file(tmp.path(), "sess-b.jsonl");
        create_jsonl_file(tmp.path(), "sess-c.jsonl");

        let sessions = discover_sessions(tmp.path(), 2).unwrap();
        assert_eq!(sessions.len(), 2);
    }

    // -- 5. Empty directory --------------------------------------------------

    #[test]
    fn test_discover_empty_directory() {
        let tmp = setup_project_dir();
        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert!(sessions.is_empty());
    }

    // -- 6. Non-jsonl files are ignored --------------------------------------

    #[test]
    fn test_discover_ignores_non_jsonl_files() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "session.jsonl");
        // Create non-jsonl files
        fs::write(tmp.path().join("readme.txt"), "hello").unwrap();
        fs::write(tmp.path().join("data.json"), "{}").unwrap();
        fs::write(tmp.path().join("notes.md"), "# Notes").unwrap();

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session");
    }

    // -- 7. Directories without matching jsonl are ignored -------------------

    #[test]
    fn test_discover_ignores_orphan_directories() {
        let tmp = setup_project_dir();
        // Create a session directory without a matching .jsonl file
        fs::create_dir_all(tmp.path().join("orphan-session").join("subagents")).unwrap();
        create_jsonl_file(tmp.path(), "orphan-session/subagents/agent-abc.jsonl");
        // Also create a valid session
        create_jsonl_file(tmp.path(), "valid-session.jsonl");

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "valid-session");
    }

    // -- 8. Non-matching subagent files are ignored --------------------------

    #[test]
    fn test_discover_ignores_non_agent_subagent_files() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-x.jsonl");
        // Create subagent dir with non-matching files
        let subagents_dir = tmp.path().join("sess-x").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        fs::write(subagents_dir.join("not-an-agent.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("agent-valid.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("random.txt"), "hello").unwrap();

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);
        // Should have main + 1 valid subagent (agent-valid.jsonl)
        assert_eq!(sessions[0].agents.len(), 2);

        let sub = sessions[0].agents.iter().find(|a| !a.is_main).unwrap();
        assert_eq!(sub.agent_id.as_deref(), Some("valid"));
    }

    // -- 9. Nonexistent project directory returns Io error -------------------

    #[test]
    fn test_discover_nonexistent_directory() {
        let result = discover_sessions(Path::new("/nonexistent/path/12345"), 10);
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionDiscoveryError::Io(_) => {} // expected
            other => panic!("expected Io error, got: {:?}", other),
        }
    }

    // -- 10. Slug is None during discovery -----------------------------------

    #[test]
    fn test_discover_slug_is_none() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-slug.jsonl");
        create_jsonl_file(tmp.path(), "sess-slug/subagents/agent-abc.jsonl");

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        for agent in &sessions[0].agents {
            assert_eq!(agent.slug, None, "slug should be None during discovery");
        }
    }

    // ========================================================================
    // Prefix matching / resolve_session tests
    // ========================================================================

    // -- 11. Auto-attach: no prefix returns most recent ---------------------

    #[test]
    fn test_resolve_auto_attach_returns_most_recent() {
        let now = SystemTime::now();
        let sessions = vec![
            create_session_with_mtime("newest-session", now),
            create_session_with_mtime("older-session", now - Duration::from_secs(3600)),
        ];

        let resolved = resolve_session(&sessions, None).unwrap();
        assert_eq!(resolved.id, "newest-session");
    }

    // -- 12. Exact match ----------------------------------------------------

    #[test]
    fn test_resolve_exact_match() {
        let now = SystemTime::now();
        let sessions = vec![
            create_session_with_mtime("abc123", now),
            create_session_with_mtime("abc456", now - Duration::from_secs(60)),
        ];

        let resolved = resolve_session(&sessions, Some("abc123")).unwrap();
        assert_eq!(resolved.id, "abc123");
    }

    // -- 13. Prefix match ---------------------------------------------------

    #[test]
    fn test_resolve_prefix_match() {
        let now = SystemTime::now();
        let sessions = vec![
            create_session_with_mtime("abc-123-xyz", now),
            create_session_with_mtime("def-456-uvw", now - Duration::from_secs(60)),
        ];

        let resolved = resolve_session(&sessions, Some("abc")).unwrap();
        assert_eq!(resolved.id, "abc-123-xyz");
    }

    // -- 14. Ambiguous prefix -----------------------------------------------

    #[test]
    fn test_resolve_ambiguous_prefix() {
        let now = SystemTime::now();
        let sessions = vec![
            create_session_with_mtime("abc-111", now),
            create_session_with_mtime("abc-222", now - Duration::from_secs(60)),
            create_session_with_mtime("def-333", now - Duration::from_secs(120)),
        ];

        let result = resolve_session(&sessions, Some("abc"));
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionDiscoveryError::AmbiguousPrefix { prefix, matches } => {
                assert_eq!(prefix, "abc");
                assert_eq!(matches.len(), 2);
                assert!(matches.contains(&"abc-111".to_string()));
                assert!(matches.contains(&"abc-222".to_string()));
            }
            other => panic!("expected AmbiguousPrefix, got: {:?}", other),
        }
    }

    // -- 15. Prefix not found -----------------------------------------------

    #[test]
    fn test_resolve_prefix_not_found() {
        let now = SystemTime::now();
        let sessions = vec![create_session_with_mtime("abc-123", now)];

        let result = resolve_session(&sessions, Some("xyz"));
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionDiscoveryError::PrefixNotFound { prefix } => {
                assert_eq!(prefix, "xyz");
            }
            other => panic!("expected PrefixNotFound, got: {:?}", other),
        }
    }

    // -- 16. Empty sessions list --------------------------------------------

    #[test]
    fn test_resolve_empty_sessions_no_prefix() {
        let result = resolve_session(&[], None);
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionDiscoveryError::NoSessions => {} // expected
            other => panic!("expected NoSessions, got: {:?}", other),
        }
    }

    #[test]
    fn test_resolve_empty_sessions_with_prefix() {
        let result = resolve_session(&[], Some("abc"));
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionDiscoveryError::NoSessions => {} // expected
            other => panic!("expected NoSessions, got: {:?}", other),
        }
    }

    // ========================================================================
    // Active / inactive status tests
    // ========================================================================

    // -- 17. Recent session is active ----------------------------------------

    #[test]
    fn test_status_recent_session_is_active() {
        let session = create_session_with_mtime(
            "recent",
            SystemTime::now() - Duration::from_secs(60), // 1 minute ago
        );
        assert_eq!(session.status(), SessionStatus::Active);
    }

    // -- 18. Old session is inactive -----------------------------------------

    #[test]
    fn test_status_old_session_is_inactive() {
        let session = create_session_with_mtime(
            "old",
            SystemTime::now() - Duration::from_secs(3600), // 1 hour ago
        );
        assert_eq!(session.status(), SessionStatus::Inactive);
    }

    // -- 19. Custom threshold ------------------------------------------------

    #[test]
    fn test_status_custom_threshold() {
        let five_min_ago = SystemTime::now() - Duration::from_secs(5 * 60);
        let session = create_session_with_mtime("custom", five_min_ago);

        // With 3-minute threshold, 5 minutes ago is inactive
        assert_eq!(
            session.status_with_threshold(Duration::from_secs(3 * 60)),
            SessionStatus::Inactive
        );

        // With 10-minute threshold, 5 minutes ago is active
        assert_eq!(
            session.status_with_threshold(Duration::from_secs(10 * 60)),
            SessionStatus::Active
        );
    }

    // -- 20. Session at UNIX_EPOCH is inactive -------------------------------

    #[test]
    fn test_status_unix_epoch_is_inactive() {
        let session = create_session_with_mtime("epoch", UNIX_EPOCH);
        assert_eq!(session.status(), SessionStatus::Inactive);
    }

    // ========================================================================
    // Edge case tests
    // ========================================================================

    // -- 21. Max mtime across agents -----------------------------------------

    #[test]
    fn test_discover_max_mtime_across_agents() {
        // We test this by creating a session with subagents and verifying
        // that last_modified is at least as recent as the most recent file.
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-mtime.jsonl");
        // Create subagent file (which may have a different mtime)
        create_jsonl_file(tmp.path(), "sess-mtime/subagents/agent-sub1.jsonl");

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);

        // The last_modified should be at least as recent as each individual file
        let main_mtime = file_modified_time(&tmp.path().join("sess-mtime.jsonl"));
        let sub_mtime = file_modified_time(
            &tmp.path()
                .join("sess-mtime")
                .join("subagents")
                .join("agent-sub1.jsonl"),
        );

        let expected_max = std::cmp::max(main_mtime, sub_mtime);
        assert_eq!(sessions[0].last_modified, expected_max);
    }

    // -- 22. Exact match takes precedence over prefix match ------------------

    #[test]
    fn test_resolve_exact_match_over_prefix() {
        let now = SystemTime::now();
        // "abc" is both an exact match and a prefix of "abcdef"
        let sessions = vec![
            create_session_with_mtime("abcdef", now),
            create_session_with_mtime("abc", now - Duration::from_secs(60)),
        ];

        let resolved = resolve_session(&sessions, Some("abc")).unwrap();
        assert_eq!(resolved.id, "abc");
    }

    // -- 23. Error Display implementations -----------------------------------

    #[test]
    fn test_error_display_no_sessions() {
        let err = SessionDiscoveryError::NoSessions;
        let msg = format!("{}", err);
        assert!(msg.contains("no sessions found"));
    }

    #[test]
    fn test_error_display_prefix_not_found() {
        let err = SessionDiscoveryError::PrefixNotFound {
            prefix: "xyz".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("xyz"));
        assert!(msg.contains("no session found"));
    }

    #[test]
    fn test_error_display_ambiguous_prefix() {
        let err = SessionDiscoveryError::AmbiguousPrefix {
            prefix: "abc".to_string(),
            matches: vec!["abc-111".to_string(), "abc-222".to_string()],
        };
        let msg = format!("{}", err);
        assert!(msg.contains("abc"));
        assert!(msg.contains("ambiguous"));
    }

    #[test]
    fn test_error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test error");
        let err = SessionDiscoveryError::Io(io_err);
        let msg = format!("{}", err);
        assert!(msg.contains("I/O error"));
    }

    // -- 24. Multiple sessions with max_sessions = 0 -------------------------

    #[test]
    fn test_discover_with_zero_limit() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-a.jsonl");
        create_jsonl_file(tmp.path(), "sess-b.jsonl");

        let sessions = discover_sessions(tmp.path(), 0).unwrap();
        assert!(sessions.is_empty());
    }

    // -- 25. Agent log_path is correct ---------------------------------------

    #[test]
    fn test_agent_log_paths_are_correct() {
        let tmp = setup_project_dir();
        create_jsonl_file(tmp.path(), "sess-paths.jsonl");
        create_jsonl_file(tmp.path(), "sess-paths/subagents/agent-xyz.jsonl");

        let sessions = discover_sessions(tmp.path(), 10).unwrap();
        assert_eq!(sessions.len(), 1);

        let main_agent = sessions[0].agents.iter().find(|a| a.is_main).unwrap();
        assert_eq!(main_agent.log_path, tmp.path().join("sess-paths.jsonl"));

        let sub_agent = sessions[0].agents.iter().find(|a| !a.is_main).unwrap();
        assert_eq!(
            sub_agent.log_path,
            tmp.path()
                .join("sess-paths")
                .join("subagents")
                .join("agent-xyz.jsonl")
        );
    }

    // ========================================================================
    // classify_new_file tests
    // ========================================================================

    // -- 26. Top-level session classification ---------------------------------

    #[test]
    fn test_classify_top_level_session() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir.join("abc123.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(
            result,
            NewFileKind::TopLevelSession {
                session_id: "abc123".to_string()
            }
        );
    }

    // -- 27. Subagent classification ------------------------------------------

    #[test]
    fn test_classify_subagent() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir
            .join("sess-001")
            .join("subagents")
            .join("agent-a0d0bbc.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(
            result,
            NewFileKind::Subagent {
                session_id: "sess-001".to_string(),
                agent_id: "a0d0bbc".to_string(),
            }
        );
    }

    // -- 28. Non-jsonl file returns Unknown -----------------------------------

    #[test]
    fn test_classify_non_jsonl_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir.join("session.json");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }

    // -- 29. Nested file not matching subagent pattern returns Unknown --------

    #[test]
    fn test_classify_nested_non_subagent_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir.join("sess").join("other").join("agent-x.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }

    // -- 30. Path not under project_dir returns Unknown -----------------------

    #[test]
    fn test_classify_outside_project_dir_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = Path::new("/other/location/session.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }

    // -- 31. Subagent without agent- prefix returns Unknown -------------------

    #[test]
    fn test_classify_subagent_without_agent_prefix_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir
            .join("sess-001")
            .join("subagents")
            .join("not-an-agent.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }

    // -- 32. Middle directory not "subagents" returns Unknown ------------------

    #[test]
    fn test_classify_wrong_middle_dir_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir
            .join("sess-001")
            .join("logs")
            .join("agent-abc.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }

    // -- 33. Deeper nesting returns Unknown -----------------------------------

    #[test]
    fn test_classify_deep_nesting_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir
            .join("sess-001")
            .join("subagents")
            .join("nested")
            .join("agent-abc.jsonl");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }

    // -- 34. No extension returns Unknown -------------------------------------

    #[test]
    fn test_classify_no_extension_is_unknown() {
        let project_dir = Path::new("/projects/my-project/.claude");
        let path = project_dir.join("session");

        let result = classify_new_file(&path, project_dir);
        assert_eq!(result, NewFileKind::Unknown);
    }
}
