use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Monitor Claude Code sessions in real-time
#[derive(Parser, Debug)]
#[command(name = "cc-tail", about = "Monitor Claude Code sessions in real-time")]
pub struct Cli {
    /// Path to the project directory (actual code path, not log path).
    /// cc-tail converts internally to the ~/.claude/projects/ equivalent.
    #[arg(long)]
    pub project: Option<PathBuf>,

    /// Attach to a specific session UUID (prefix match supported).
    /// Default: auto-attach to the most recently active session.
    #[arg(long)]
    pub session: Option<String>,

    /// Show progress entries and additional metadata.
    /// Writes debug info to stderr.
    #[arg(long, default_value_t = false)]
    pub verbose: bool,

    /// Color theme: dark or light
    #[arg(long, value_enum)]
    pub theme: Option<Theme>,

    /// Path to config file
    #[arg(long)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Lightweight streaming mode that tails a single JSONL file to stdout
    Stream(StreamArgs),
}

#[derive(Args, Debug)]
pub struct StreamArgs {
    /// Path to a specific .jsonl file to tail
    #[arg(long)]
    pub file: PathBuf,

    /// Number of visible messages to replay from the file before live tailing
    #[arg(long, default_value_t = 20)]
    pub replay: usize,

    /// Show progress entries and parse errors
    #[arg(long, default_value_t = false)]
    pub verbose: bool,

    /// Color theme for ANSI output
    #[arg(long, value_enum)]
    pub theme: Option<Theme>,
}

#[derive(Clone, Debug, PartialEq, ValueEnum)]
pub enum Theme {
    Dark,
    Light,
}

impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Theme::Dark => write!(f, "dark"),
            Theme::Light => write!(f, "light"),
        }
    }
}
