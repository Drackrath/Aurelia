use super::*;

/// Parse an argv into a `Cli` the way the binary does.
fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).expect("args should parse")
}

mod parse;
mod parse_display;
mod progress;
mod routing;
mod snapshots;
