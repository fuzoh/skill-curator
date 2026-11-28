//! Command-line surface: argument parsing and dispatch only.
//!
//! Parsing stays thin; each command delegates to library modules that own the
//! behaviour.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::Error;
use crate::collection::Collection;
use crate::materialize::{self, Options};
use crate::rewrite::Agent;
use crate::safety::{HomeDir, Name, PathError};
use crate::source::{Git, SourceCache};

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
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Report Collection drift, Destination drift, and known upstream updates.
    Status,
    /// Fetch Sources and classify upstream changes without modifying any state.
    Check,
    /// Move Skills to new Revisions, reviewing agent-assisted changes one Skill
    /// at a time.
    Update(UpdateArgs),
    /// Replay local Adaptation and Composition changes at the pinned Revision,
    /// without fetching.
    Materialize(MaterializeArgs),
    /// Copy accepted Skills and assembled Fragments to the enabled
    /// Destinations.
    Install,
}

/// Choices for Skills whose files in `skills/<name>/` were edited by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct MaterializeArgs {
    /// Accept the manual edits of Skill SKILL into lock.toml (repeatable).
    #[arg(long, value_name = "SKILL", value_parser = parse_name)]
    pub accept: Vec<Name>,
    /// Discard the manual edits of Skill SKILL and rebuild its deterministic
    /// Adaptation at the pinned Revision (repeatable).
    #[arg(long, value_name = "SKILL", value_parser = parse_name)]
    pub reset: Vec<Name>,
}

/// Options of `update`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct UpdateArgs {
    /// Who performs agent-assisted rewrites.
    #[command(flatten)]
    pub agent: AgentArgs,
}

/// Choice of the agent that performs agent-assisted rewrites. Without a
/// flag, the `agent` setting of machine.toml applies, else `pi`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct AgentArgs {
    /// Agent that performs agent-assisted rewrites, started in each staging
    /// directory [default: the `agent` setting of machine.toml, else pi]
    #[arg(long, value_enum, value_name = "AGENT")]
    pub agent: Option<Agent>,
    /// Perform agent-assisted rewrites yourself: print each staging directory
    /// and wait for confirmation instead of starting an agent (same as
    /// `--agent manual`)
    #[arg(long, conflicts_with = "agent")]
    pub manual: bool,
}

impl AgentArgs {
    /// The agent chosen on the command line, if any.
    #[must_use]
    pub fn choice(&self) -> Option<Agent> {
        if self.manual {
            Some(Agent::Manual)
        } else {
            self.agent
        }
    }
}

fn parse_name(raw: &str) -> Result<Name, PathError> {
    Name::new(raw)
}

impl Command {
    /// Name of the command as typed on the command line.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Check => "check",
            Self::Update(_) => "update",
            Self::Materialize(_) => "materialize",
            Self::Install => "install",
        }
    }
}

/// Executes the parsed command line.
///
/// # Errors
///
/// Returns an [`Error`] when the command fails or leaves Skills needing
/// attention. Commands without behaviour yet report
/// [`Error::NotImplemented`].
pub fn run(cli: &Cli) -> Result<(), Error> {
    match &cli.command {
        Command::Materialize(args) => run_materialize(cli, args),
        command => Err(Error::NotImplemented {
            command: command.name(),
        }),
    }
}

fn run_materialize(cli: &Cli, args: &MaterializeArgs) -> Result<(), Error> {
    let root = collection_root(cli)?;
    let cache = source_cache()?;
    let collection = Collection::load(&root)?;
    let options = Options {
        accept: args.accept.clone(),
        reset: args.reset.clone(),
    };
    let report = materialize::materialize(&collection, &cache, &options)?;
    for (name, outcome) in &report.skills {
        println!("{}", outcome.describe(name));
    }
    match report.pending() {
        0 => Ok(()),
        count => Err(Error::Unresolved { count }),
    }
}

/// The Collection directory: `--collection`, or the working directory.
fn collection_root(cli: &Cli) -> Result<PathBuf, Error> {
    match &cli.collection {
        Some(root) => Ok(root.clone()),
        None => std::env::current_dir().map_err(|err| Error::Environment {
            message: format!("cannot read the working directory: {err}; pass `--collection`"),
        }),
    }
}

/// The Source cache under `$XDG_CACHE_HOME`, or `~/.cache`.
fn source_cache() -> Result<SourceCache, Error> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| Error::Environment {
            message: "`HOME` is not set; it locates the Source cache".to_owned(),
        })
        .and_then(|home| {
            HomeDir::new(home).map_err(|err| Error::Environment {
                message: format!("`HOME` cannot locate the Source cache: {err}"),
            })
        })?;
    let root = SourceCache::default_root(std::env::var_os("XDG_CACHE_HOME").as_deref(), &home);
    Ok(SourceCache::new(root, Git::system()))
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
            Command::Update(UpdateArgs::default()),
            Command::Materialize(MaterializeArgs::default()),
            Command::Install,
        ]
        .map(|command| command.name().to_owned());
        assert_eq!(parsed, named);
    }

    #[test]
    fn run_reports_the_command_as_not_implemented() {
        let cli = Cli::try_parse_from(["skill-curator", "status"]).unwrap();
        assert!(matches!(
            run(&cli),
            Err(Error::NotImplemented { command: "status" })
        ));
    }

    #[test]
    fn update_chooses_the_agent() {
        let choice = |args: &[&str]| {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::Update(update) = cli.command else {
                panic!("not update");
            };
            update.agent.choice()
        };
        assert_eq!(choice(&["skill-curator", "update"]), None);
        assert_eq!(
            choice(&["skill-curator", "update", "--agent", "claude"]),
            Some(Agent::Claude)
        );
        assert_eq!(
            choice(&["skill-curator", "update", "--agent", "pi"]),
            Some(Agent::Pi)
        );
        assert_eq!(
            choice(&["skill-curator", "update", "--manual"]),
            Some(Agent::Manual)
        );
        assert_eq!(
            choice(&["skill-curator", "update", "--agent", "manual"]),
            Some(Agent::Manual)
        );
        assert!(Cli::try_parse_from(["skill-curator", "update", "--agent", "codex"]).is_err());
        assert!(
            Cli::try_parse_from(["skill-curator", "update", "--manual", "--agent", "pi"]).is_err()
        );
    }

    #[test]
    fn materialize_takes_repeatable_skill_names() {
        let cli = Cli::try_parse_from([
            "skill-curator",
            "materialize",
            "--accept",
            "grilling",
            "--reset",
            "code-review",
            "--accept",
            "tdd",
        ])
        .unwrap();
        let name = |raw| Name::new(raw).unwrap();
        assert_eq!(
            cli.command,
            Command::Materialize(MaterializeArgs {
                accept: vec![name("grilling"), name("tdd")],
                reset: vec![name("code-review")],
            })
        );
        assert!(Cli::try_parse_from(["skill-curator", "materialize", "--reset", "../x"]).is_err());
    }
}
