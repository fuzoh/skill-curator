//! Command-line surface: argument parsing and dispatch only.
//!
//! Parsing stays thin; each command delegates to library modules that own the
//! behaviour.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::Error;

/// Download, adapt, and maintain third-party agent Skills in a user-owned
/// Collection.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "skill-curator", version, arg_required_else_help = true)]
pub struct Cli {
    /// Collection directory holding the Manifest, lock, intents, Skills, and
    /// Fragments.
    #[arg(long, global = true, value_name = "PATH")]
    pub collection: Option<PathBuf>,

    /// Machine-local file that enables, disables, or relocates Destinations on
    /// this machine.
    #[arg(long, global = true, value_name = "PATH")]
    pub machine_config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

/// The v1 commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Report Collection drift, Destination drift, and known upstream updates.
    Status,
    /// Fetch Sources and classify upstream changes without modifying any state.
    Check,
    /// Move Skills to new Revisions, reviewing agent-assisted changes one Skill
    /// at a time.
    Update,
    /// Replay local Adaptation and Composition changes at the pinned Revision,
    /// without fetching.
    Materialize,
    /// Copy accepted Skills and assembled Fragments to the enabled
    /// Destinations.
    Install,
}

impl Command {
    /// Name of the command as typed on the command line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Check => "check",
            Self::Update => "update",
            Self::Materialize => "materialize",
            Self::Install => "install",
        }
    }
}

/// Executes the parsed command line.
///
/// # Errors
///
/// Returns an [`Error`] when the command fails; in this release every command
/// reports [`Error::NotImplemented`].
pub fn run(cli: &Cli) -> Result<(), Error> {
    Err(Error::NotImplemented {
        command: cli.command.name(),
    })
}

/// Process entry point: parses `std::env::args`, runs the command, and maps
/// the outcome to an exit code. Usage errors exit through the argument parser
/// with code `2`.
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("skill-curator: error: {err}");
            err.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_global_flags_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "skill-curator",
            "install",
            "--collection",
            "/tmp/collection",
            "--machine-config",
            "/tmp/machine.toml",
        ])
        .unwrap();
        assert_eq!(cli.command, Command::Install);
        assert_eq!(cli.collection, Some(PathBuf::from("/tmp/collection")));
        assert_eq!(cli.machine_config, Some(PathBuf::from("/tmp/machine.toml")));
    }

    #[test]
    fn subcommand_names_match_the_parser() {
        let parsed = Cli::command()
            .get_subcommands()
            .map(|sub| sub.get_name().to_owned())
            .collect::<Vec<_>>();
        let named = [
            Command::Status,
            Command::Check,
            Command::Update,
            Command::Materialize,
            Command::Install,
        ]
        .map(|command| command.name().to_owned());
        assert_eq!(parsed, named);
    }

    #[test]
    fn run_reports_the_command_as_not_implemented() {
        let cli = Cli::try_parse_from(["skill-curator", "materialize"]).unwrap();
        assert_eq!(
            run(&cli),
            Err(Error::NotImplemented {
                command: "materialize"
            })
        );
    }
}
