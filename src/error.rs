use std::fmt;
use std::process::ExitCode;

use crate::collection::CollectionError;
use crate::install::InstallError;
use crate::materialize::MaterializeError;

/// Failure of a `skill-curator` command after its arguments were accepted.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The named command is part of the v1 surface but has no behaviour yet.
    NotImplemented {
        /// Command name as typed on the command line.
        command: &'static str,
    },
    /// The process environment does not provide what the command needs,
    /// such as an absolute `HOME`.
    Environment {
        /// What is missing or wrong.
        message: String,
    },
    /// The Collection could not be loaded.
    Collection(CollectionError),
    /// `materialize` could not run.
    Materialize(MaterializeError),
    /// `install` could not run.
    Install(InstallError),
    /// The command finished, but Skills still need the user's attention; each
    /// was reported on standard output.
    Unresolved {
        /// Number of Skill outcomes still needing attention.
        count: usize,
    },
    /// `install` finished, but paths in Destinations still need the user's
    /// attention; each was reported on standard output and left untouched.
    InstallUnresolved {
        /// Number of paths still needing attention.
        count: usize,
    },
}

impl Error {
    /// Process exit code reported for this error.
    ///
    /// Exit codes: `0` success, `1` command failure or Skills left needing
    /// attention, `2` usage error reported by the argument parser.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::NotImplemented { .. }
            | Self::Environment { .. }
            | Self::Collection(_)
            | Self::Materialize(_)
            | Self::Install(_)
            | Self::Unresolved { .. }
            | Self::InstallUnresolved { .. } => ExitCode::FAILURE,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented { command } => {
                write!(f, "the `{command}` command is not implemented yet")
            }
            Self::Environment { message } => f.write_str(message),
            Self::Collection(err) => err.fmt(f),
            Self::Materialize(err) => err.fmt(f),
            Self::Install(err) => err.fmt(f),
            Self::Unresolved { count: 1 } => f.write_str("1 Skill outcome needs attention"),
            Self::Unresolved { count } => write!(f, "{count} Skill outcomes need attention"),
            Self::InstallUnresolved { count: 1 } => {
                f.write_str("1 path in a Destination needs attention; it was left untouched")
            }
            Self::InstallUnresolved { count } => write!(
                f,
                "{count} paths in Destinations need attention; they were left untouched"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Collection(err) => Some(err),
            Self::Materialize(err) => Some(err),
            Self::Install(err) => Some(err),
            _ => None,
        }
    }
}

impl From<CollectionError> for Error {
    fn from(err: CollectionError) -> Self {
        Self::Collection(err)
    }
}

impl From<InstallError> for Error {
    fn from(err: InstallError) -> Self {
        Self::Install(err)
    }
}

impl From<MaterializeError> for Error {
    fn from(err: MaterializeError) -> Self {
        Self::Materialize(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_names_the_command() {
        let err = Error::NotImplemented { command: "status" };
        assert_eq!(
            err.to_string(),
            "the `status` command is not implemented yet"
        );
        assert_eq!(err.exit_code(), ExitCode::FAILURE);
    }

    #[test]
    fn unresolved_counts_the_outcomes_and_fails() {
        assert_eq!(
            Error::Unresolved { count: 1 }.to_string(),
            "1 Skill outcome needs attention"
        );
        let err = Error::Unresolved { count: 3 };
        assert_eq!(err.to_string(), "3 Skill outcomes need attention");
        assert_eq!(err.exit_code(), ExitCode::FAILURE);
    }

    #[test]
    fn install_unresolved_counts_the_targets_and_fails() {
        assert_eq!(
            Error::InstallUnresolved { count: 1 }.to_string(),
            "1 path in a Destination needs attention; it was left untouched"
        );
        let err = Error::InstallUnresolved { count: 2 };
        assert_eq!(
            err.to_string(),
            "2 paths in Destinations need attention; they were left untouched"
        );
        assert_eq!(err.exit_code(), ExitCode::FAILURE);
    }
}
