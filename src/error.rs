use std::fmt;
use std::process::ExitCode;

/// Failure of a `skill-curator` command after its arguments were accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The named command is part of the v1 surface but has no behaviour yet.
    NotImplemented {
        /// Command name as typed on the command line.
        command: &'static str,
    },
}

impl Error {
    /// Process exit code reported for this error.
    ///
    /// Exit codes: `0` success, `1` command failure, `2` usage error reported
    /// by the argument parser.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::NotImplemented { .. } => ExitCode::FAILURE,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented { command } => {
                write!(f, "the `{command}` command is not implemented yet")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_implemented_names_the_command() {
        let err = Error::NotImplemented { command: "install" };
        assert_eq!(
            err.to_string(),
            "the `install` command is not implemented yet"
        );
        assert_eq!(err.exit_code(), ExitCode::FAILURE);
    }
}
