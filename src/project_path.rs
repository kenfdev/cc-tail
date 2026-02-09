//! Auto-detection of Claude Code project directories.
//!
//! Claude Code stores per-project configuration and logs under
//! `~/.claude/projects/<escaped-path>/`. This module converts a working
//! directory (or an explicit `--project` override) into the matching
//! escaped directory name and locates it on disk.
//!
//! The detection uses a 5-level strategy (see [`detect_project_path`]):
//! 1. Explicit `--project` override
//! 2. Exact CWD match
//! 3. Parent-directory walk (most specific ancestor wins)
//! 4. Git repository root fallback
//! 5. Error with all searched paths

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when detecting the Claude Code project directory.
#[derive(Debug)]
pub enum ProjectDetectionError {
    /// No matching project directory found under ~/.claude/projects/
    NotFound { searched_paths: Vec<PathBuf> },
    /// Could not determine the user's home directory
    NoHomeDir,
}

impl fmt::Display for ProjectDetectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectDetectionError::NotFound { searched_paths } => {
                write!(
                    f,
                    "no matching project directory found under ~/.claude/projects/. Searched: {:?}. \
                     Use --project to specify the project path explicitly.",
                    searched_paths
                )
            }
            ProjectDetectionError::NoHomeDir => {
                write!(f, "could not determine the user's home directory")
            }
        }
    }
}

impl std::error::Error for ProjectDetectionError {}

// ---------------------------------------------------------------------------
// Path escaping
// ---------------------------------------------------------------------------

/// Escape a filesystem path into Claude Code's project directory name format.
///
/// Replaces every `/`, `.`, ` ` (space), and `~` with a `-` (hyphen).
/// The result is the directory name used under `~/.claude/projects/`.
fn escape_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    // Normalize trailing slash before escaping
    let normalized = s.trim_end_matches('/');
    // If the path was just "/", normalized is now empty; use the original
    let input = if normalized.is_empty() {
        &*s
    } else {
        normalized
    };
    input
        .chars()
        .map(|c| match c {
            '/' | '.' | ' ' | '~' => '-',
            other => other,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Base directory
// ---------------------------------------------------------------------------

/// Return the base path `~/.claude/projects/`.
fn claude_projects_base() -> Result<PathBuf, ProjectDetectionError> {
    let home = dirs::home_dir().ok_or(ProjectDetectionError::NoHomeDir)?;
    Ok(home.join(".claude").join("projects"))
}

// ---------------------------------------------------------------------------
// Directory lookup helpers
// ---------------------------------------------------------------------------

/// Check if `base/escaped` is an existing directory.
/// Returns `Some(base/escaped)` if it exists, `None` otherwise.
fn find_project_dir(base: &Path, escaped: &str) -> Option<PathBuf> {
    let candidate = base.join(escaped);
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Run `git rev-parse --show-toplevel` in `cwd` and return the result.
/// Returns `None` if git is not installed, not a repo, or the command fails.
fn git_root(cwd: &Path) -> Option<PathBuf> {
    Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout);
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(trimmed))
                }
            } else {
                None
            }
        })
}

// ---------------------------------------------------------------------------
// Core detection logic (testable via dependency injection)
// ---------------------------------------------------------------------------

/// Testable core of `detect_project_path`.
///
/// `base` is the `~/.claude/projects/` directory.
/// `git_root_fn` is injected so tests can avoid calling real `git`.
fn detect_project_path_with_base<F>(
    base: &Path,
    cwd: &Path,
    explicit_project: Option<&Path>,
    git_root_fn: F,
) -> Result<PathBuf, ProjectDetectionError>
where
    F: Fn(&Path) -> Option<PathBuf>,
{
    let mut searched_paths: Vec<PathBuf> = Vec::new();

    // --- Strategy 1: explicit --project override ---
    if let Some(proj) = explicit_project {
        let canonical = proj.canonicalize().unwrap_or_else(|_| proj.to_path_buf());
        let escaped = escape_path(&canonical);
        searched_paths.push(base.join(&escaped));
        if let Some(found) = find_project_dir(base, &escaped) {
            return Ok(found);
        }
        // If explicit project was specified but not found, fail immediately
        return Err(ProjectDetectionError::NotFound { searched_paths });
    }

    // --- Strategy 2: exact CWD match ---
    let canonical_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let escaped_cwd = escape_path(&canonical_cwd);
    searched_paths.push(base.join(&escaped_cwd));
    if let Some(found) = find_project_dir(base, &escaped_cwd) {
        return Ok(found);
    }

    // --- Strategy 3: parent-walk strategy ---
    // Walk up the directory tree, collecting all matches. Longest path wins.
    let mut best_match: Option<PathBuf> = None;
    let mut best_depth: usize = 0;

    for ancestor in canonical_cwd.ancestors().skip(1) {
        // skip(1) because we already checked exact cwd
        if ancestor == Path::new("") || ancestor == Path::new("/") {
            // Skip root; escape_path("/") -> "-" which is unlikely to be meaningful,
            // but we still record it for error reporting.
            let escaped = escape_path(ancestor);
            searched_paths.push(base.join(&escaped));
            break;
        }

        let escaped = escape_path(ancestor);
        let candidate_path = base.join(&escaped);

        if candidate_path.is_dir() {
            // Count the depth (number of components) to pick the longest/most specific match
            let depth = ancestor.components().count();
            if depth > best_depth {
                best_depth = depth;
                best_match = Some(candidate_path);
            }
        }
        searched_paths.push(base.join(&escaped));
    }

    if let Some(found) = best_match {
        return Ok(found);
    }

    // --- Strategy 4: git root fallback ---
    if let Some(root) = git_root_fn(cwd) {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let escaped = escape_path(&canonical_root);
        let candidate_path = base.join(&escaped);
        if !searched_paths.contains(&candidate_path) {
            searched_paths.push(candidate_path.clone());
        }
        if let Some(found) = find_project_dir(base, &escaped) {
            return Ok(found);
        }
    }

    // --- Strategy 5: nothing found ---
    Err(ProjectDetectionError::NotFound { searched_paths })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect the Claude Code project directory for the given working directory.
///
/// Returns the path to the project directory under `~/.claude/projects/`.
///
/// Detection strategy (in priority order):
/// 1. Explicit `--project` override: canonicalize, escape, check exists
/// 2. Exact CWD match: canonicalize cwd, escape, check exists
/// 3. Parent-walk: walk up parents, collect matches, longest path wins
/// 4. Git root fallback: `git rev-parse --show-toplevel`, escape, check exists
/// 5. Return error with all searched paths
pub fn detect_project_path(
    cwd: &Path,
    explicit_project: Option<&Path>,
) -> Result<PathBuf, ProjectDetectionError> {
    let base = claude_projects_base()?;

    if !base.is_dir() {
        return Err(ProjectDetectionError::NotFound {
            searched_paths: vec![base],
        });
    }

    detect_project_path_with_base(&base, cwd, explicit_project, git_root)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // -- escape_path tests ---------------------------------------------------

    #[test]
    fn test_escape_simple_path() {
        let path = Path::new("/Users/fukuyamaken/ghq/github.com/kenfdev/cc-tail");
        assert_eq!(
            escape_path(path),
            "-Users-fukuyamaken-ghq-github-com-kenfdev-cc-tail"
        );
    }

    #[test]
    fn test_escape_dots_in_path() {
        let path = Path::new("/home/user/my.project/src");
        assert_eq!(escape_path(path), "-home-user-my-project-src");
    }

    #[test]
    fn test_escape_spaces_in_path() {
        let path = Path::new("/home/user/my project/src");
        assert_eq!(escape_path(path), "-home-user-my-project-src");
    }

    #[test]
    fn test_escape_tilde_in_path() {
        let path = Path::new("~/my-project");
        assert_eq!(escape_path(path), "--my-project");
    }

    #[test]
    fn test_escape_hyphens_preserved() {
        let path = Path::new("/home/user/my-project");
        assert_eq!(escape_path(path), "-home-user-my-project");
    }

    #[test]
    fn test_escape_root_path() {
        let path = Path::new("/");
        assert_eq!(escape_path(path), "-");
    }

    #[test]
    fn test_escape_trailing_slash() {
        // Path::new normalizes trailing slashes, so "/foo/bar/" == "/foo/bar"
        let path = Path::new("/foo/bar/");
        assert_eq!(escape_path(path), "-foo-bar");
    }

    #[test]
    fn test_escape_multiple_dots() {
        let path = Path::new("/home/user/.config/some.thing.else");
        assert_eq!(escape_path(path), "-home-user--config-some-thing-else");
    }

    #[test]
    fn test_escape_mixed_special_chars() {
        let path = Path::new("/Users/john doe/my.project/~backup");
        assert_eq!(escape_path(path), "-Users-john-doe-my-project--backup");
    }

    // -- detect_project_path_with_base tests ---------------------------------

    /// Helper to create a fake Claude projects base with specific project dirs
    fn setup_projects_base(project_dirs: &[&str]) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for dir in project_dirs {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        tmp
    }

    /// A git_root_fn that always returns None (no git)
    fn no_git(_cwd: &Path) -> Option<PathBuf> {
        None
    }

    #[test]
    fn test_explicit_override_found() {
        // The explicit project path /foo/bar escapes to -foo-bar
        let base_dir = setup_projects_base(&["-foo-bar"]);

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/some/other/cwd"),
            Some(Path::new("/foo/bar")),
            no_git,
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-foo-bar"));
    }

    #[test]
    fn test_explicit_override_not_found() {
        let base_dir = setup_projects_base(&[]);

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/some/cwd"),
            Some(Path::new("/nonexistent/path")),
            no_git,
        );

        assert!(result.is_err());
        match result.unwrap_err() {
            ProjectDetectionError::NotFound { searched_paths } => {
                assert!(!searched_paths.is_empty());
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_exact_cwd_match() {
        // cwd = /foo/bar -> escaped = -foo-bar
        let base_dir = setup_projects_base(&["-foo-bar"]);

        let result =
            detect_project_path_with_base(base_dir.path(), Path::new("/foo/bar"), None, no_git);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-foo-bar"));
    }

    #[test]
    fn test_parent_walk_finds_parent() {
        // cwd = /foo/bar/baz/qux but only /foo/bar exists as a project
        // escaped /foo/bar -> -foo-bar
        let base_dir = setup_projects_base(&["-foo-bar"]);

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/foo/bar/baz/qux"),
            None,
            no_git,
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-foo-bar"));
    }

    #[test]
    fn test_parent_walk_most_specific_wins() {
        // Both /foo and /foo/bar exist as project dirs
        // cwd = /foo/bar/baz
        // The more specific /foo/bar should win over /foo
        let base_dir = setup_projects_base(&["-foo", "-foo-bar"]);

        let result =
            detect_project_path_with_base(base_dir.path(), Path::new("/foo/bar/baz"), None, no_git);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-foo-bar"));
    }

    #[test]
    fn test_git_root_fallback() {
        // cwd has no match, but git root does
        let base_dir = setup_projects_base(&["-git-repo-root"]);

        let git_fn = |_cwd: &Path| -> Option<PathBuf> { Some(PathBuf::from("/git/repo/root")) };

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/some/random/path"),
            None,
            git_fn,
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-git-repo-root"));
    }

    #[test]
    fn test_nothing_found_returns_error() {
        let base_dir = setup_projects_base(&[]);

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/no/match/anywhere"),
            None,
            no_git,
        );

        assert!(result.is_err());
        match result.unwrap_err() {
            ProjectDetectionError::NotFound { searched_paths } => {
                assert!(!searched_paths.is_empty(), "should list searched paths");
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_base_dir_missing() {
        // When the base directory itself doesn't exist
        let tmp = TempDir::new().unwrap();
        let nonexistent_base = tmp.path().join("nonexistent");

        let result =
            detect_project_path_with_base(&nonexistent_base, Path::new("/foo/bar"), None, no_git);

        // find_project_dir checks is_dir which returns false for nonexistent paths
        assert!(result.is_err());
    }

    #[test]
    fn test_explicit_overrides_exact_match() {
        // Both the explicit project and the cwd exist, explicit should win
        let base_dir = setup_projects_base(&["-explicit-project", "-current-cwd"]);

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/current/cwd"),
            Some(Path::new("/explicit/project")),
            no_git,
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-explicit-project"));
    }

    #[test]
    fn test_exact_match_overrides_git_fallback() {
        // Both exact CWD and git root match, exact CWD wins (higher priority)
        let base_dir = setup_projects_base(&["-exact-cwd", "-git-root"]);

        let git_fn = |_cwd: &Path| -> Option<PathBuf> { Some(PathBuf::from("/git/root")) };

        let result =
            detect_project_path_with_base(base_dir.path(), Path::new("/exact/cwd"), None, git_fn);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-exact-cwd"));
    }

    #[test]
    fn test_parent_walk_overrides_git_fallback() {
        // Parent walk finds a match, so git root should NOT be used
        let base_dir = setup_projects_base(&["-parent", "-git-root"]);

        let git_fn = |_cwd: &Path| -> Option<PathBuf> { Some(PathBuf::from("/git/root")) };

        let result = detect_project_path_with_base(
            base_dir.path(),
            Path::new("/parent/child/grandchild"),
            None,
            git_fn,
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), base_dir.path().join("-parent"));
    }

    #[test]
    fn test_git_root_not_available() {
        // Git root returns None (git not installed or not a repo), nothing matches
        let base_dir = setup_projects_base(&[]);

        let result =
            detect_project_path_with_base(base_dir.path(), Path::new("/no/match"), None, no_git);

        assert!(result.is_err());
    }

    #[test]
    fn test_error_display_not_found() {
        let err = ProjectDetectionError::NotFound {
            searched_paths: vec![PathBuf::from("/a"), PathBuf::from("/b")],
        };
        let msg = format!("{}", err);
        assert!(msg.contains("no matching project directory found"));
        assert!(msg.contains("--project"));
    }

    #[test]
    fn test_error_display_no_home_dir() {
        let err = ProjectDetectionError::NoHomeDir;
        let msg = format!("{}", err);
        assert!(msg.contains("home directory"));
    }

    #[test]
    fn test_escape_path_with_real_world_example() {
        // The example from the plan
        let path = Path::new("/Users/fukuyamaken/ghq/github.com/kenfdev/cc-tail");
        let escaped = escape_path(path);
        assert_eq!(escaped, "-Users-fukuyamaken-ghq-github-com-kenfdev-cc-tail");
    }

    #[test]
    fn test_canonicalize_on_real_cwd_match() {
        // Use tempdir to create a real directory that can be canonicalized.
        // On macOS, tempdir paths (e.g. /var/folders/...) may be symlinks to
        // /private/var/folders/..., so we must use the canonicalized path for
        // the project dir name, since detect_project_path_with_base canonicalizes.
        let base_dir = TempDir::new().unwrap();
        let real_cwd = TempDir::new().unwrap();

        // Canonicalize to get the real path (resolves symlinks)
        let canonical_cwd = real_cwd.path().canonicalize().unwrap();
        let escaped = escape_path(&canonical_cwd);
        fs::create_dir_all(base_dir.path().join(&escaped)).unwrap();

        let result = detect_project_path_with_base(base_dir.path(), real_cwd.path(), None, no_git);

        assert!(result.is_ok());
    }
}
