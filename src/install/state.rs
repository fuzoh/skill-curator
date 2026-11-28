//! Ownership state of `install`: the Skill directories and global instruction
//! files `skill-curator` wrote into Destinations, with the hash of what it
//! wrote.
//!
//! The state lets a later run tell managed files from unmanaged ones (written
//! by hand or by another tool such as `npx skills`), prune managed Skills that
//! are no longer declared, and detect manual edits made in a Destination. It
//! lives in one machine-local file under `$XDG_STATE_HOME/skill-curator/`
//! (`~/.local/state/skill-curator/` by default), never inside a Destination,
//! where harnesses would scan it.
//!
//! ```toml
//! format = 1
//!
//! [skills_dirs."/home/user/.claude/skills"]
//! destination = "claude"
//! skills = { grill-me = "sha256:…" }
//!
//! [instruction_files."/home/user/.claude/CLAUDE.md"]
//! destination = "claude"
//! hash = "sha256:…"
//! ```

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::write_file;
use crate::hash::Digest;
use crate::safety::Name;

/// State format written and read by this release.
pub const STATE_FORMAT: u32 = 1;
/// State file name inside the state directory.
pub const STATE_FILE: &str = "install.toml";

/// Parsed install state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallState {
    /// State format; only [`STATE_FORMAT`] is understood.
    pub format: u32,
    /// Managed Skills, by absolute Skills directory of a Destination.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills_dirs: BTreeMap<PathBuf, ManagedSkills>,
    /// Managed global instruction files, by absolute path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub instruction_files: BTreeMap<PathBuf, ManagedFile>,
}

impl Default for InstallState {
    fn default() -> Self {
        Self {
            format: STATE_FORMAT,
            skills_dirs: BTreeMap::new(),
            instruction_files: BTreeMap::new(),
        }
    }
}

/// The Skills `skill-curator` installed in one Skills directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedSkills {
    /// Destination that last installed here, for messages.
    pub destination: Name,
    /// Hash of each installed Skill directory, by Skill name.
    #[serde(default)]
    pub skills: BTreeMap<Name, Digest>,
}

/// A global instruction file `skill-curator` assembled and installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedFile {
    /// Destination that last installed it, for messages.
    pub destination: Name,
    /// Hash of the installed contents.
    pub hash: Digest,
}

/// Failure to read or write the install state.
#[derive(Debug)]
#[non_exhaustive]
pub enum StateError {
    /// The state file could not be read.
    Read {
        /// State file.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The state file is not valid TOML or does not match its schema.
    Parse {
        /// State file.
        path: PathBuf,
        /// TOML error.
        source: Box<toml::de::Error>,
    },
    /// The state file parsed but holds something this release refuses.
    Invalid {
        /// State file.
        path: PathBuf,
        /// What is wrong.
        problems: Vec<String>,
    },
    /// The state file could not be written.
    Write {
        /// State file.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "cannot read the install state {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(f, "invalid install state {}: {source}", path.display())
            }
            Self::Invalid { path, problems } => {
                write!(f, "invalid install state {}:", path.display())?;
                for problem in problems {
                    write!(f, "\n  - {problem}")?;
                }
                Ok(())
            }
            Self::Write { path, source } => {
                write!(
                    f,
                    "cannot write the install state {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source.as_ref()),
            Self::Invalid { .. } => None,
        }
    }
}

impl InstallState {
    /// Reads the state at `path`, or returns an empty state when the file
    /// does not exist yet.
    ///
    /// # Errors
    ///
    /// Returns a [`StateError`] when the file cannot be read or parsed, was
    /// written in another format, or records a path that is not absolute,
    /// has a `..` component, or is the filesystem root.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(StateError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let state: Self = toml::from_str(&text).map_err(|source| StateError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        let problems = state.problems();
        if problems.is_empty() {
            Ok(state)
        } else {
            Err(StateError::Invalid {
                path: path.to_path_buf(),
                problems,
            })
        }
    }

    fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.format != STATE_FORMAT {
            problems.push(format!(
                "state format {} is not supported by this release, which reads format {STATE_FORMAT}; upgrade skill-curator",
                self.format
            ));
        }
        let paths = self
            .skills_dirs
            .keys()
            .map(|path| ("skills_dirs", path))
            .chain(
                self.instruction_files
                    .keys()
                    .map(|path| ("instruction_files", path)),
            );
        for (table, path) in paths {
            if !is_confined(path) {
                problems.push(format!(
                    "{table}: `{}` must be an absolute path below the filesystem root, without `..`",
                    path.display()
                ));
            }
        }
        problems
    }

    /// Writes the state to `path` atomically, creating its directory.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Write`] when the directory or file cannot be
    /// written.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        let write_error = |source| StateError::Write {
            path: path.to_path_buf(),
            source,
        };
        let text = toml::to_string(self).map_err(|err| write_error(io::Error::other(err)))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(write_error)?;
        }
        write_file(path, text.as_bytes(), true).map_err(write_error)
    }

    /// Hash recorded for Skill `name` in `skills_dir`.
    #[must_use]
    pub fn skill(&self, skills_dir: &Path, name: &Name) -> Option<&Digest> {
        self.skills_dirs.get(skills_dir)?.skills.get(name)
    }

    /// Records Skill `name` as installed in `skills_dir` with `digest`.
    pub fn record_skill(
        &mut self,
        destination: &Name,
        skills_dir: &Path,
        name: &Name,
        digest: Digest,
    ) {
        let managed = self
            .skills_dirs
            .entry(skills_dir.to_path_buf())
            .or_insert_with(|| ManagedSkills {
                destination: destination.clone(),
                skills: BTreeMap::new(),
            });
        managed.destination = destination.clone();
        managed.skills.insert(name.clone(), digest);
    }

    /// Forgets Skill `name` in `skills_dir`, and the directory once it holds
    /// no managed Skill.
    pub fn forget_skill(&mut self, skills_dir: &Path, name: &Name) {
        if let Some(managed) = self.skills_dirs.get_mut(skills_dir) {
            managed.skills.remove(name);
            if managed.skills.is_empty() {
                self.skills_dirs.remove(skills_dir);
            }
        }
    }

    /// Hash recorded for the instruction file at `path`.
    #[must_use]
    pub fn instruction_file(&self, path: &Path) -> Option<&Digest> {
        self.instruction_files
            .get(path)
            .map(|managed| &managed.hash)
    }

    /// Records the instruction file at `path` as installed with `digest`.
    pub fn record_instruction_file(&mut self, destination: &Name, path: &Path, digest: Digest) {
        self.instruction_files.insert(
            path.to_path_buf(),
            ManagedFile {
                destination: destination.clone(),
                hash: digest,
            },
        );
    }

    /// Forgets the instruction file at `path`.
    pub fn forget_instruction_file(&mut self, path: &Path) {
        self.instruction_files.remove(path);
    }
}

/// Whether `path` is absolute, has no `..` component, and is not the root.
fn is_confined(path: &Path) -> bool {
    path.is_absolute()
        && path.parent().is_some()
        && !path.components().any(|c| c == Component::ParentDir)
}

/// Default location of the install state: `$XDG_STATE_HOME/skill-curator/`
/// when that variable holds an absolute path, `$HOME/.local/state/skill-curator/`
/// otherwise.
///
/// Returns `None` when neither variable gives an absolute directory.
#[must_use]
pub fn default_state_path(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let state_home = absolute_dir(xdg_state_home)
        .map(Path::to_path_buf)
        .or_else(|| absolute_dir(home).map(|home| home.join(".local").join("state")))?;
    Some(state_home.join("skill-curator").join(STATE_FILE))
}

fn absolute_dir(value: Option<&OsStr>) -> Option<&Path> {
    value.map(Path::new).filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::hash::file_digest;

    fn name(raw: &str) -> Name {
        Name::new(raw).unwrap()
    }

    #[test]
    fn missing_state_is_empty_and_saving_round_trips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/skill-curator").join(STATE_FILE);
        assert_eq!(InstallState::load(&path).unwrap(), InstallState::default());

        let mut state = InstallState::default();
        let skills = Path::new("/home/user/.claude/skills");
        state.record_skill(
            &name("claude"),
            skills,
            &name("grill-me"),
            file_digest(b"a"),
        );
        state.record_instruction_file(
            &name("claude"),
            Path::new("/home/user/.claude/CLAUDE.md"),
            file_digest(b"b"),
        );
        state.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("format = 1\n"), "{text}");
        assert!(
            text.contains("[skills_dirs.\"/home/user/.claude/skills\"]"),
            "{text}"
        );
        let loaded = InstallState::load(&path).unwrap();
        assert_eq!(loaded, state);
        assert_eq!(
            loaded.skill(skills, &name("grill-me")),
            Some(&file_digest(b"a"))
        );
        assert_eq!(
            loaded.instruction_file(Path::new("/home/user/.claude/CLAUDE.md")),
            Some(&file_digest(b"b"))
        );
    }

    #[test]
    fn forgetting_the_last_skill_drops_its_directory() {
        let mut state = InstallState::default();
        let skills = Path::new("/srv/skills");
        state.record_skill(&name("shared"), skills, &name("a"), file_digest(b"a"));
        state.record_skill(&name("shared"), skills, &name("b"), file_digest(b"b"));
        state.forget_skill(skills, &name("a"));
        assert_eq!(state.skills_dirs[skills].skills.len(), 1);
        state.forget_skill(skills, &name("b"));
        assert!(state.skills_dirs.is_empty());
        state.forget_skill(skills, &name("b"));

        let file = Path::new("/srv/AGENTS.md");
        state.record_instruction_file(&name("shared"), file, file_digest(b""));
        state.forget_instruction_file(file);
        assert_eq!(state, InstallState::default());
    }

    #[test]
    fn refuses_other_formats_and_unconfined_paths() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(STATE_FILE);
        let digest = file_digest(b"x");
        for (text, expected) in [
            ("format = 2\n", "state format 2 is not supported"),
            (
                &*format!(
                    "format = 1\n[skills_dirs.\"relative/skills\"]\ndestination = \"d\"\nskills = {{ a = \"{digest}\" }}\n"
                ),
                "`relative/skills` must be an absolute path",
            ),
            (
                &*format!(
                    "format = 1\n[instruction_files.\"/home/u/../x\"]\ndestination = \"d\"\nhash = \"{digest}\"\n"
                ),
                "`/home/u/../x` must be an absolute path",
            ),
            (
                &*format!(
                    "format = 1\n[skills_dirs.\"/\"]\ndestination = \"d\"\nskills = {{ etc = \"{digest}\" }}\n"
                ),
                "`/` must be an absolute path below the filesystem root",
            ),
        ] {
            fs::write(&path, text).unwrap();
            let err = InstallState::load(&path).unwrap_err().to_string();
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
        for (text, expected) in [
            ("format = 1\nother = true\n", "unknown field `other`"),
            (
                "format = 1\n[skills_dirs.\"/s\"]\ndestination = \"d\"\nskills = { \"../x\" = \"sha256:00\" }\n",
                "not a valid",
            ),
        ] {
            fs::write(&path, text).unwrap();
            let err = InstallState::load(&path).unwrap_err().to_string();
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
    }

    #[test]
    fn default_path_follows_xdg_then_home() {
        let path = |xdg: Option<&str>, home: Option<&str>| {
            default_state_path(xdg.map(OsStr::new), home.map(OsStr::new))
        };
        assert_eq!(
            path(Some("/xdg"), Some("/home/u")),
            Some(PathBuf::from("/xdg/skill-curator/install.toml"))
        );
        assert_eq!(
            path(None, Some("/home/u")),
            Some(PathBuf::from(
                "/home/u/.local/state/skill-curator/install.toml"
            ))
        );
        assert_eq!(
            path(Some("relative"), Some("/home/u")),
            Some(PathBuf::from(
                "/home/u/.local/state/skill-curator/install.toml"
            ))
        );
        assert_eq!(path(None, None), None);
        assert_eq!(path(None, Some("relative")), None);
    }
}
