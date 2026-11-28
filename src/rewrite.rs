//! Agent-assisted rewrites: hands an Adaptation or Composition rewrite to the
//! user's agent and validates the Skill it proposes.
//!
//! The tool never calls a model API. It prepares a temporary staging
//! directory ([`Staging`]), lets an agent (`pi` by default, or `claude`) or
//! the user ([`Agent::Manual`]) write the proposed Skill into its `output/`
//! directory, then validates that directory, enforces the D1 frontmatter
//! rules again, and diffs the result against the accepted Skill
//! (`accepted_local`) for review. Nothing is written to the Collection here;
//! persisting an accepted [`Proposal`] is the caller's decision.
//!
//! ```text
//! <staging>/
//! ├── BRIEF.md              instructions: intent, D1 rules, upstream changes
//! ├── base/                 det(R_old) of an Adaptation, when reachable
//! ├── upstream/             det(R_new) of an Adaptation
//! ├── inputs/<input>/       post-D2 view of each Composition input at R_new
//! ├── diffs/<name>.diff     R_old -> R_new diff (Adaptation: upstream.diff)
//! ├── current/              accepted_local, when the Skill was accepted before
//! └── output/               writable; pre-populated, receives the proposed Skill
//! ```
//!
//! Everything but `output/` is read-only. Files downloaded from a Source are
//! data for the agent to read; the brief tells it never to run them.

mod brief;
mod output;
mod runner;
mod staging;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::adaptation::{AdaptationError, SkillTree};
use crate::collection::manifest::FrontmatterRules;
use crate::diff::TreeDiff;
use crate::safety::Name;

pub use output::OutputError;
pub use runner::{AGENT_PROMPT, Agent, Runner};
pub use staging::{
    BASE_DIR, BRIEF_FILE, CURRENT_DIR, DIFFS_DIR, INPUTS_DIR, OUTPUT_DIR, Staging, UPSTREAM_DIR,
};

/// What an agent rewrite starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inputs {
    /// A single-Source Adaptation.
    Adaptation {
        /// `det(R_old)`: the deterministic adaptation at the Revision the
        /// accepted Skill was produced from. `None` when that Revision is
        /// unreachable or the Skill was never accepted, which degrades the
        /// rewrite to a two-way comparison.
        base: Option<SkillTree>,
        /// `det(R_new)`: the deterministic adaptation at the new Revision.
        upstream: SkillTree,
    },
    /// A Composition, by input name.
    Composition(BTreeMap<Name, InputRevisions>),
}

/// One Composition input at the old and the new Revision, after its D2 rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRevisions {
    /// The input at the Revision the accepted Skill was produced from, when
    /// reachable.
    pub old: Option<SkillTree>,
    /// The input at the new Revision.
    pub new: SkillTree,
}

/// One rewrite job for the Skill `skill`.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// The Skill being rewritten.
    pub skill: Name,
    /// The upstream views the rewrite starts from.
    pub inputs: Inputs,
    /// `accepted_local`: the accepted Skill in `skills/<name>/`, if any.
    pub current: Option<SkillTree>,
    /// Text of the written intent (`intents/<skill>.md`).
    pub intent: Option<String>,
    /// D1 frontmatter rule sets, in application order, enforced again on
    /// the proposed Skill.
    pub frontmatter: Vec<FrontmatterRules>,
    /// Extra guidance from the user when a rewrite is run again.
    pub guidance: Option<String>,
}

/// The Skill an agent proposed, validated and with D1 enforced again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// The proposed Skill.
    pub skill: SkillTree,
    /// Changes from `accepted_local` (or from nothing) to the proposed Skill.
    pub diff: TreeDiff,
    /// Whether enforcing the D1 rules changed the agent's `SKILL.md`, for
    /// example because it edited a key the rules set or remove.
    pub frontmatter_enforced: bool,
}

/// Failure to prepare, run, or collect an agent rewrite.
#[derive(Debug)]
#[non_exhaustive]
pub enum RewriteError {
    /// The staging directory could not be prepared.
    Staging {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// A view of a Skill could not be written into the staging directory.
    View(AdaptationError),
    /// The agent program could not be started.
    Spawn {
        /// The agent.
        agent: Agent,
        /// Program that was run.
        program: OsString,
        /// Underlying error.
        source: io::Error,
    },
    /// The agent exited with a failure status; its output is discarded.
    AgentFailed {
        /// The agent.
        agent: Agent,
        /// How it exited.
        status: ExitStatus,
    },
    /// The user stopped a manual rewrite instead of confirming it.
    Aborted,
    /// The terminal could not be read or written.
    Terminal(io::Error),
    /// The proposed Skill in `output/` was rejected.
    Output(OutputError),
}

impl fmt::Display for RewriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Staging { path, source } => write!(
                f,
                "cannot prepare the staging directory {}: {source}",
                path.display()
            ),
            Self::View(err) => write!(f, "cannot prepare the staging directory: {err}"),
            Self::Spawn {
                agent,
                program,
                source,
            } => write!(
                f,
                "cannot start {agent} (`{}`): {source}; install it, choose another agent, or use `--manual`",
                program.to_string_lossy()
            ),
            Self::AgentFailed { agent, status } => write!(
                f,
                "{agent} failed ({status}); its proposed Skill was discarded"
            ),
            Self::Aborted => f.write_str("the rewrite was stopped; nothing was accepted"),
            Self::Terminal(err) => write!(f, "cannot use the terminal: {err}"),
            Self::Output(err) => write!(f, "the proposed Skill was rejected: {err}"),
        }
    }
}

impl std::error::Error for RewriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Staging { source, .. } | Self::Spawn { source, .. } => Some(source),
            Self::Terminal(err) => Some(err),
            Self::View(err) => Some(err),
            Self::Output(err) => Some(err),
            Self::AgentFailed { .. } | Self::Aborted => None,
        }
    }
}

impl From<OutputError> for RewriteError {
    fn from(err: OutputError) -> Self {
        Self::Output(err)
    }
}

impl From<AdaptationError> for RewriteError {
    fn from(err: AdaptationError) -> Self {
        Self::View(err)
    }
}

/// Runs one rewrite: prepares the staging directory for `job`, hands it to
/// `runner`, then validates and diffs the proposed Skill. The staging
/// directory is removed when this returns, whatever the outcome.
///
/// `terminal_in` and `terminal_out` carry the exchange with the user:
/// progress messages, and the confirmation awaited in manual mode.
///
/// # Errors
///
/// Returns a [`RewriteError`] when the staging directory cannot be
/// prepared, the agent cannot be started or fails, the user stops a manual
/// rewrite, or the proposed Skill is rejected.
pub fn rewrite(
    job: Job,
    runner: &Runner,
    terminal_in: &mut dyn BufRead,
    terminal_out: &mut dyn Write,
) -> Result<Proposal, RewriteError> {
    let staging = Staging::create(job)?;
    runner.run(&staging, terminal_in, terminal_out)?;
    staging.collect()
}
