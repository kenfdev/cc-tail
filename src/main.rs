use cctail::cli::{Cli, Commands};
use cctail::config::build_config;
use cctail::stream;
use cctail::tui;
use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let app_config = build_config(&cli);

    if app_config.verbose {
        eprintln!("cc-tail: effective config: {:?}", app_config);
    }

    match cli.command {
        Some(Commands::Stream(ref stream_args)) => {
            if app_config.verbose {
                eprintln!(
                    "cc-tail: stream mode: file={}, replay={}, verbose={}, theme={:?}",
                    stream_args.file.display(),
                    stream_args.replay,
                    stream_args.verbose,
                    stream_args.theme
                );
            }
            if let Err(e) = stream::run_stream(stream_args).await {
                eprintln!("cc-tail: stream error: {}", e);
                std::process::exit(1);
            }
        }
        None => {
            if app_config.verbose {
                eprintln!("cc-tail: TUI mode");
            }
            if let Err(e) = tui::run_tui(app_config) {
                eprintln!("cc-tail: TUI error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
