//! Invocation of the system `git` binary.
//!
//! Every command runs with repository hooks, the file system monitor, the
//! `ext::` transport, submodule recursion, and automatic garbage collection
//! disabled, with replace objects ignored, without standard input, and with
//! the environment variables that would redirect Git to another repository
//! removed. Nothing downloaded from a Source is ever executed.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use super::SourceError;

/// Configuration forced on every `git` invocation, overriding the user's
/// own settings.
const SAFE_CONFIG: &[&str] = &[
    // No hook ever runs, including `reference-transaction` during a fetch.
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    // `ext::` URLs run arbitrary commands.
    "protocol.ext.allow=never",
    "submodule.recurse=false",
    "fetch.recurseSubmodules=false",
    // Commits that upstream history no longer reaches stay in the cache, so
    // a pinned Revision survives a rewrite it was fetched before.
    "gc.auto=0",
    "maintenance.auto=false",
];

/// Environment variables that point Git at another repository, index, object
/// store, or configuration. They are set, for example, when the tool runs
/// from inside a Git hook.
const REPOSITORY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_QUARANTINE_PATH",
    "GIT_TEMPLATE_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

/// The `git` program used to fetch and read Sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    program: OsString,
    env: Vec<(OsString, OsString)>,
}

impl Default for Git {
    fn default() -> Self {
        Self::system()
    }
}

impl Git {
    /// The `git` found on `PATH`.
    #[must_use]
    pub fn system() -> Self {
        Self::with_program("git")
    }

    /// A specific `git` program: a name looked up on `PATH` or a path.
    #[must_use]
    pub fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            env: Vec::new(),
        }
    }

    /// Adds an environment variable to every invocation, for example
    /// `GIT_CONFIG_GLOBAL` to isolate the tool from the user's settings.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Runs `git <args>`, inside the repository `git_dir` when given, and
    /// returns its standard output.
    pub(crate) fn run(
        &self,
        git_dir: Option<&Path>,
        args: &[&OsStr],
    ) -> Result<Vec<u8>, SourceError> {
        let mut command = Command::new(&self.program);
        for var in REPOSITORY_ENV {
            command.env_remove(var);
        }
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
        command.arg("--no-replace-objects");
        for setting in SAFE_CONFIG {
            command.arg("-c").arg(setting);
        }
        if let Some(dir) = git_dir {
            let mut option = OsString::from("--git-dir=");
            option.push(dir);
            command.arg(option);
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = command.output().map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                SourceError::GitMissing {
                    program: self.program.clone(),
                }
            } else {
                SourceError::GitSpawn {
                    program: self.program.clone(),
                    source,
                }
            }
        })?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(SourceError::Git {
                command: describe(args),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

/// The Git subcommand of `args`, for diagnostics.
fn describe(args: &[&OsStr]) -> String {
    let subcommand = args
        .first()
        .map(|arg| arg.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("git {subcommand}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_is_reported_as_missing_git() {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::with_program(dir.path().join("no-such-git"));
        let err = git.run(None, &[OsStr::new("--version")]).unwrap_err();
        assert!(matches!(err, SourceError::GitMissing { .. }), "{err:?}");
    }

    #[test]
    fn failures_name_the_subcommand_and_keep_git_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let err = Git::system()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .run(
                Some(&dir.path().join("absent.git")),
                &[OsStr::new("rev-parse"), OsStr::new("HEAD")],
            )
            .unwrap_err();
        let SourceError::Git { command, stderr } = err else {
            panic!("unexpected error {err:?}");
        };
        assert_eq!(command, "git rev-parse");
        assert!(!stderr.is_empty());
    }
}
