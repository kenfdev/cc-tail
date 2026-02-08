use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::cli::{Cli, Theme};

// ---------------------------------------------------------------------------
// TOML-deserializable config (intermediate representation)
// ---------------------------------------------------------------------------

/// Raw config as parsed from the TOML file.
/// All fields are optional so that missing keys fall through to defaults.
/// Unknown keys are silently ignored by serde.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct FileConfig {
    verbose: Option<bool>,
    theme: Option<String>,
    display: FileDisplayConfig,
    tmux: FileTmuxConfig,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct FileDisplayConfig {
    timestamps: Option<bool>,
    timestamp_format: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct FileTmuxConfig {
    session_prefix: Option<String>,
    layout: Option<String>,
}

// ---------------------------------------------------------------------------
// Effective (merged) config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub project: Option<PathBuf>,
    pub session: Option<String>,
    pub verbose: bool,
    pub theme: Theme,
    pub display: DisplayConfig,
    pub tmux: TmuxConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayConfig {
    pub timestamps: bool,
    pub timestamp_format: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TmuxConfig {
    pub session_prefix: String,
    pub layout: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            project: None,
            session: None,
            verbose: false,
            theme: Theme::Dark,
            display: DisplayConfig::default(),
            tmux: TmuxConfig::default(),
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            timestamps: true,
            timestamp_format: "%H:%M:%S".to_string(),
        }
    }
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            session_prefix: "cc-tail".to_string(),
            layout: "tiled".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Returns the default config file path: `~/.config/cc-tail/config.toml`
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cc-tail").join("config.toml"))
}

/// Load the config file from the given path (or the default path).
/// Returns the parsed `FileConfig`, or `None` if the file does not exist
/// or cannot be parsed.
fn load_file_config(path: &Path) -> Option<FileConfig> {
    if !path.exists() {
        return None;
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<FileConfig>(&contents) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                eprintln!(
                    "cc-tail: warning: failed to parse config file {}: {}",
                    path.display(),
                    e
                );
                None
            }
        },
        Err(e) => {
            eprintln!(
                "cc-tail: warning: failed to read config file {}: {}",
                path.display(),
                e
            );
            None
        }
    }
}

/// Parse a theme string from the config file into a `Theme` enum.
/// Returns `None` if the string is not recognized (caller uses default).
fn parse_theme(s: &str) -> Option<Theme> {
    match s.to_lowercase().as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        other => {
            eprintln!(
                "cc-tail: warning: unknown theme \"{}\", using default",
                other
            );
            None
        }
    }
}

/// Build the effective `AppConfig` by merging defaults, config file, and CLI args.
///
/// Precedence (highest wins):
/// 1. CLI flags (if explicitly provided)
/// 2. Config file values
/// 3. Hardcoded defaults
pub fn build_config(cli: &Cli) -> AppConfig {
    // Step 1: Start with defaults
    let mut config = AppConfig::default();

    // Step 2: Determine config file path
    let config_path = cli.config.clone().or_else(default_config_path);

    // Step 3: Load and overlay config file
    if let Some(ref path) = config_path {
        if let Some(file_cfg) = load_file_config(path) {
            // Overlay file config onto defaults
            if let Some(v) = file_cfg.verbose {
                config.verbose = v;
            }
            if let Some(ref t) = file_cfg.theme {
                if let Some(theme) = parse_theme(t) {
                    config.theme = theme;
                }
            }
            if let Some(ts) = file_cfg.display.timestamps {
                config.display.timestamps = ts;
            }
            if let Some(ref fmt) = file_cfg.display.timestamp_format {
                config.display.timestamp_format = fmt.clone();
            }
            if let Some(ref prefix) = file_cfg.tmux.session_prefix {
                config.tmux.session_prefix = prefix.clone();
            }
            if let Some(ref layout) = file_cfg.tmux.layout {
                config.tmux.layout = layout.clone();
            }
        } else if cli.config.is_some() {
            // User explicitly specified --config but file could not be loaded.
            // The warning was already printed by load_file_config if the file
            // existed but was malformed. If the file didn't exist at all,
            // print a warning here.
            if !path.exists() {
                eprintln!(
                    "cc-tail: warning: config file not found: {}",
                    path.display()
                );
            }
        }
    }

    // Step 4: CLI overrides
    if cli.project.is_some() {
        config.project = cli.project.clone();
    }
    if cli.session.is_some() {
        config.session = cli.session.clone();
    }
    if cli.verbose {
        config.verbose = true;
    }
    if let Some(ref theme) = cli.theme {
        config.theme = theme.clone();
    }

    config
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper: parse a TOML string into a FileConfig
    fn parse_file_config(toml_str: &str) -> Option<FileConfig> {
        toml::from_str::<FileConfig>(toml_str).ok()
    }

    /// Helper: write TOML to a temp file and load it
    fn load_from_string(toml_str: &str) -> Option<FileConfig> {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(toml_str.as_bytes()).unwrap();
        load_file_config(f.path())
    }

    /// Helper: build a minimal Cli struct for testing
    fn default_cli() -> Cli {
        Cli {
            project: None,
            session: None,
            verbose: false,
            theme: None,
            config: None,
            command: None,
        }
    }

    // -- Default config tests -------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.project, None);
        assert_eq!(config.session, None);
        assert!(!config.verbose);
        assert_eq!(config.theme, Theme::Dark);
        assert!(config.display.timestamps);
        assert_eq!(config.display.timestamp_format, "%H:%M:%S");
        assert_eq!(config.tmux.session_prefix, "cc-tail");
        assert_eq!(config.tmux.layout, "tiled");
    }

    // -- TOML parsing tests ---------------------------------------------------

    #[test]
    fn test_parse_valid_full_config() {
        let toml = r#"
verbose = true
theme = "light"

[display]
timestamps = false
timestamp_format = "%Y-%m-%d %H:%M:%S"

[tmux]
session_prefix = "my-prefix"
layout = "even-horizontal"
"#;
        let cfg = parse_file_config(toml).unwrap();
        assert_eq!(cfg.verbose, Some(true));
        assert_eq!(cfg.theme.as_deref(), Some("light"));
        assert_eq!(cfg.display.timestamps, Some(false));
        assert_eq!(
            cfg.display.timestamp_format.as_deref(),
            Some("%Y-%m-%d %H:%M:%S")
        );
        assert_eq!(cfg.tmux.session_prefix.as_deref(), Some("my-prefix"));
        assert_eq!(cfg.tmux.layout.as_deref(), Some("even-horizontal"));
    }

    #[test]
    fn test_parse_empty_config() {
        let cfg = parse_file_config("").unwrap();
        assert_eq!(cfg.verbose, None);
        assert_eq!(cfg.theme, None);
        assert_eq!(cfg.display.timestamps, None);
        assert_eq!(cfg.display.timestamp_format, None);
        assert_eq!(cfg.tmux.session_prefix, None);
        assert_eq!(cfg.tmux.layout, None);
    }

    #[test]
    fn test_parse_partial_config_missing_keys() {
        let toml = r#"
verbose = true

[display]
timestamps = false
"#;
        let cfg = parse_file_config(toml).unwrap();
        assert_eq!(cfg.verbose, Some(true));
        assert_eq!(cfg.theme, None);
        assert_eq!(cfg.display.timestamps, Some(false));
        assert_eq!(cfg.display.timestamp_format, None);
        assert_eq!(cfg.tmux.session_prefix, None);
        assert_eq!(cfg.tmux.layout, None);
    }

    #[test]
    fn test_unknown_keys_ignored() {
        let toml = r#"
verbose = false
unknown_key = "should be ignored"
another_unknown = 42

[display]
timestamps = true
fancy_mode = true

[unknown_section]
foo = "bar"
"#;
        let cfg = parse_file_config(toml).unwrap();
        assert_eq!(cfg.verbose, Some(false));
        assert!(cfg.display.timestamps == Some(true));
    }

    #[test]
    fn test_malformed_toml_returns_none() {
        let toml = r#"
this is not valid toml [[[
"#;
        let result = parse_file_config(toml);
        assert!(result.is_none());
    }

    #[test]
    fn test_load_missing_file() {
        let path = Path::new("/tmp/cc-tail-test-nonexistent-config-12345.toml");
        let result = load_file_config(path);
        assert!(result.is_none());
    }

    #[test]
    fn test_load_valid_file() {
        let toml = r#"
verbose = true
theme = "dark"
"#;
        let cfg = load_from_string(toml).unwrap();
        assert_eq!(cfg.verbose, Some(true));
        assert_eq!(cfg.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn test_load_malformed_file() {
        let result = load_from_string("not valid {{{{ toml");
        assert!(result.is_none());
    }

    // -- Theme parsing tests --------------------------------------------------

    #[test]
    fn test_parse_theme_dark() {
        assert_eq!(parse_theme("dark"), Some(Theme::Dark));
        assert_eq!(parse_theme("Dark"), Some(Theme::Dark));
        assert_eq!(parse_theme("DARK"), Some(Theme::Dark));
    }

    #[test]
    fn test_parse_theme_light() {
        assert_eq!(parse_theme("light"), Some(Theme::Light));
        assert_eq!(parse_theme("Light"), Some(Theme::Light));
    }

    #[test]
    fn test_parse_theme_unknown() {
        assert_eq!(parse_theme("solarized"), None);
        assert_eq!(parse_theme(""), None);
    }

    // -- build_config merge tests ---------------------------------------------

    #[test]
    fn test_build_config_defaults_no_file() {
        let cli = Cli {
            config: Some(PathBuf::from("/tmp/cc-tail-nonexistent-54321.toml")),
            ..default_cli()
        };
        let config = build_config(&cli);
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn test_build_config_file_overrides_defaults() {
        let toml = r#"
verbose = true
theme = "light"

[display]
timestamps = false
timestamp_format = "%H:%M"

[tmux]
session_prefix = "custom"
layout = "even-vertical"
"#;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(toml.as_bytes()).unwrap();

        let cli = Cli {
            config: Some(f.path().to_path_buf()),
            ..default_cli()
        };
        let config = build_config(&cli);

        assert!(config.verbose);
        assert_eq!(config.theme, Theme::Light);
        assert!(!config.display.timestamps);
        assert_eq!(config.display.timestamp_format, "%H:%M");
        assert_eq!(config.tmux.session_prefix, "custom");
        assert_eq!(config.tmux.layout, "even-vertical");
    }

    #[test]
    fn test_build_config_cli_overrides_file() {
        let toml = r#"
verbose = false
theme = "light"
"#;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(toml.as_bytes()).unwrap();

        let cli = Cli {
            config: Some(f.path().to_path_buf()),
            verbose: true,
            theme: Some(Theme::Dark),
            project: Some(PathBuf::from("/my/project")),
            session: Some("abc123".to_string()),
            command: None,
        };
        let config = build_config(&cli);

        // CLI overrides
        assert!(config.verbose);
        assert_eq!(config.theme, Theme::Dark);
        assert_eq!(config.project, Some(PathBuf::from("/my/project")));
        assert_eq!(config.session, Some("abc123".to_string()));
    }

    #[test]
    fn test_build_config_cli_verbose_false_does_not_override_file() {
        // When CLI verbose is false (default), the file's verbose=true should win
        let toml = r#"
verbose = true
"#;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(toml.as_bytes()).unwrap();

        let cli = Cli {
            config: Some(f.path().to_path_buf()),
            verbose: false,
            ..default_cli()
        };
        let config = build_config(&cli);

        // verbose=false from CLI is the default, so file's true wins
        assert!(config.verbose);
    }

    #[test]
    fn test_build_config_partial_file() {
        // File only sets theme, rest should be defaults
        let toml = r#"
theme = "light"
"#;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(toml.as_bytes()).unwrap();

        let cli = Cli {
            config: Some(f.path().to_path_buf()),
            ..default_cli()
        };
        let config = build_config(&cli);

        assert!(!config.verbose); // default
        assert_eq!(config.theme, Theme::Light); // from file
        assert!(config.display.timestamps); // default
        assert_eq!(config.display.timestamp_format, "%H:%M:%S"); // default
        assert_eq!(config.tmux.session_prefix, "cc-tail"); // default
        assert_eq!(config.tmux.layout, "tiled"); // default
    }

    #[test]
    fn test_build_config_unknown_theme_in_file_uses_default() {
        let toml = r#"
theme = "solarized"
"#;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(toml.as_bytes()).unwrap();

        let cli = Cli {
            config: Some(f.path().to_path_buf()),
            ..default_cli()
        };
        let config = build_config(&cli);

        // Unknown theme falls through to default
        assert_eq!(config.theme, Theme::Dark);
    }
}
