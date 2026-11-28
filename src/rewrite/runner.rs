//! Hands a staging directory to the user's agent, or to the user.
//!
//! `pi` and `claude` are started interactively in the staging directory with
//! [`AGENT_PROMPT`] as their first message, inheriting the terminal, and the
//! rewrite continues once they exit. [`Agent::Manual`] starts nothing: it
//! prints the staging directory and waits for the user to confirm.

use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, Write};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::staging::{BRIEF_FILE, OUTPUT_DIR};
use super::{RewriteError, Staging};

/// First message given to `pi` and `claude`, started in the staging
/// directory.
pub const AGENT_PROMPT: &str = "Read BRIEF.md in the current directory and follow it: write the complete proposed Skill into output/, change nothing else, then end the session.";

/// Answer that stops a manual rewrite.
const ABORT: &str = "abort";

/// Who performs an agent-assisted rewrite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// The `pi` coding agent (the default).
    #[default]
    Pi,
    /// Claude Code (`claude`).
    Claude,
    /// The user, in the staging directory, outside skill-curator.
    Manual,
}

impl Agent {
    /// The agent chosen on the command line, else the one configured in
    /// `machine.toml`, else `pi`.
    #[must_use]
    pub fn select(command_line: Option<Self>, configured: Option<Self>) -> Self {
        command_line.or(configured).unwrap_or_default()
    }

    /// Name as written on the command line and in `machine.toml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Claude => "claude",
            Self::Manual => "manual",
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Runs an [`Agent`] against a staging directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runner {
    agent: Agent,
    program: OsString,
}

impl Runner {
    /// A runner for `agent`, starting `pi` or `claude` from `PATH`.
    #[must_use]
    pub fn new(agent: Agent) -> Self {
        let program = match agent {
            Agent::Pi => "pi",
            Agent::Claude => "claude",
            Agent::Manual => "",
        };
        Self {
            agent,
            program: program.into(),
        }
    }

    /// Starts `program` instead of the agent's usual binary. A manual runner
    /// starts nothing either way.
    #[must_use]
    pub fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> Agent {
        self.agent
    }

    /// Hands `staging` over and returns once the agent exited successfully,
    /// or the user confirmed a manual rewrite.
    ///
    /// # Errors
    ///
    /// Returns [`RewriteError::Spawn`] when the agent cannot be started,
    /// [`RewriteError::AgentFailed`] when it exits with a failure status,
    /// [`RewriteError::Aborted`] when the user stops a manual rewrite or
    /// closes its input, and [`RewriteError::Terminal`] when the terminal
    /// cannot be used.
    pub fn run(
        &self,
        staging: &Staging,
        terminal_in: &mut dyn BufRead,
        terminal_out: &mut dyn Write,
    ) -> Result<(), RewriteError> {
        if self.agent == Agent::Manual {
            return manual(staging, terminal_in, terminal_out);
        }
        writeln!(
            terminal_out,
            "Handing the rewrite of Skill `{}` to {} in {}",
            staging.skill(),
            self.agent,
            staging.path().display()
        )
        .and_then(|()| terminal_out.flush())
        .map_err(RewriteError::Terminal)?;
        let status = Command::new(&self.program)
            .arg(AGENT_PROMPT)
            .current_dir(staging.path())
            .status()
            .map_err(|source| RewriteError::Spawn {
                agent: self.agent,
                program: self.program.clone(),
                source,
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(RewriteError::AgentFailed {
                agent: self.agent,
                status,
            })
        }
    }
}

fn manual(
    staging: &Staging,
    terminal_in: &mut dyn BufRead,
    terminal_out: &mut dyn Write,
) -> Result<(), RewriteError> {
    let dir = staging.path().display();
    writeln!(
        terminal_out,
        "Staging directory for the rewrite of Skill `{}`: {dir}\n\
         Read {dir}/{BRIEF_FILE} and write the complete proposed Skill into {dir}/{OUTPUT_DIR}/.",
        staging.skill()
    )
    .map_err(RewriteError::Terminal)?;
    loop {
        write!(
            terminal_out,
            "Press Enter when the proposed Skill is ready, or type `{ABORT}` to stop: "
        )
        .and_then(|()| terminal_out.flush())
        .map_err(RewriteError::Terminal)?;
        let mut answer = String::new();
        let read = terminal_in
            .read_line(&mut answer)
            .map_err(RewriteError::Terminal)?;
        match answer.trim() {
            _ if read == 0 => return Err(RewriteError::Aborted),
            "" => return Ok(()),
            ABORT => return Err(RewriteError::Aborted),
            other => writeln!(terminal_out, "Unknown answer `{other}`.")
                .map_err(RewriteError::Terminal)?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_prefers_the_command_line_then_the_configuration() {
        assert_eq!(Agent::select(None, None), Agent::Pi);
        assert_eq!(Agent::select(None, Some(Agent::Claude)), Agent::Claude);
        assert_eq!(
            Agent::select(Some(Agent::Manual), Some(Agent::Claude)),
            Agent::Manual
        );
        assert_eq!(
            Agent::select(Some(Agent::Pi), Some(Agent::Manual)),
            Agent::Pi
        );
    }

    #[test]
    fn runners_start_the_agent_binaries_from_path() {
        assert_eq!(Runner::new(Agent::Pi).program, OsString::from("pi"));
        assert_eq!(Runner::new(Agent::Claude).program, OsString::from("claude"));
        let runner = Runner::new(Agent::Claude).with_program("/bin/mock");
        assert_eq!(runner.program, OsString::from("/bin/mock"));
        assert_eq!(runner.agent(), Agent::Claude);
    }

    #[test]
    fn names_match_the_serialised_form() {
        for agent in [Agent::Pi, Agent::Claude, Agent::Manual] {
            let toml = toml::Value::try_from(agent).unwrap();
            assert_eq!(toml.as_str(), Some(agent.as_str()));
        }
    }
}
