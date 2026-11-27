//! The user-owned Collection: loading and validating `manifest.toml` and
//! `lock.toml`, merging machine-local Destination settings, and hashing a
//! Skill's inputs and accepted output.
//!
//! ```text
//! <collection>/
//! ├── manifest.toml   portable declarations (Manifest)
//! ├── lock.toml       accepted Revisions, hashes, provenance, licenses
//! ├── intents/        written Adaptation and Composition intents
//! ├── skills/<name>/  accepted materialised Skills
//! └── fragments/      global instruction Fragments
//! ```

pub mod lock;
pub mod machine;
pub mod manifest;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::hash::{self, Digest, HashError};
use crate::safety::{HomeDir, Name, RelativePath};
use lock::{LOCK_FORMAT, Lock};
use machine::{MachineConfig, ResolvedDestination};
use manifest::{FRAGMENTS_DIR, INTENTS_DIR, Manifest, Preset, Skill};

/// Manifest file name at the Collection root.
pub const MANIFEST_FILE: &str = "manifest.toml";
/// Lock file name at the Collection root.
pub const LOCK_FILE: &str = "lock.toml";
/// Collection directory holding accepted materialised Skills.
pub const SKILLS_DIR: &str = "skills";
/// File every Skill directory must contain.
pub const SKILL_FILE: &str = "SKILL.md";

/// One validation finding: where it is (a dotted TOML key path) and what to
/// do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Dotted key path of the offending entry, such as `skills.grill.source`.
    pub location: String,
    /// What is wrong and how to fix it.
    pub message: String,
}

impl Problem {
    pub(crate) fn new(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// Failure to load or use a Collection or machine-local settings.
#[derive(Debug)]
#[non_exhaustive]
pub enum CollectionError {
    /// A file could not be read.
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// A file is not valid TOML or does not match its schema.
    Parse {
        /// File that failed to parse.
        path: PathBuf,
        /// TOML error, naming the line and key.
        source: Box<toml::de::Error>,
    },
    /// A file parsed but its content is inconsistent.
    Invalid {
        /// File the problems belong to.
        path: PathBuf,
        /// Every problem found, in file order.
        problems: Vec<Problem>,
    },
    /// The Skill is not declared in the Manifest.
    UnknownSkill {
        /// Requested Skill name.
        name: Name,
    },
    /// A Skill directory could not be hashed.
    Hash(HashError),
}

impl fmt::Display for CollectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "invalid {}: {source}", path.display()),
            Self::Invalid { path, problems } => {
                write!(f, "invalid {}:", path.display())?;
                for problem in problems {
                    write!(f, "\n  - {problem}")?;
                }
                Ok(())
            }
            Self::UnknownSkill { name } => {
                write!(f, "Skill `{name}` is not declared in {MANIFEST_FILE}")
            }
            Self::Hash(source) => source.fmt(f),
        }
    }
}

impl std::error::Error for CollectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source.as_ref()),
            Self::Hash(source) => Some(source),
            Self::Invalid { .. } | Self::UnknownSkill { .. } => None,
        }
    }
}

impl From<HashError> for CollectionError {
    fn from(source: HashError) -> Self {
        Self::Hash(source)
    }
}

/// Reads a file, returning `None` when it does not exist.
fn read_optional(path: &Path) -> Result<Option<String>, CollectionError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CollectionError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parse<T>(
    path: &Path,
    text: &str,
    from_toml: fn(&str) -> Result<T, toml::de::Error>,
) -> Result<T, CollectionError> {
    from_toml(text).map_err(|source| CollectionError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// A loaded and validated Collection.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    root: PathBuf,
    manifest: Manifest,
    lock: Lock,
}

impl Collection {
    /// Loads the Collection at `root`: parses `manifest.toml` (required) and
    /// `lock.toml` (absent in a new Collection), then validates them together
    /// with the files they reference.
    ///
    /// # Errors
    ///
    /// Returns a [`CollectionError`] when a file cannot be read or parsed, or
    /// listing every validation problem: undefined Sources or presets,
    /// Compositions without a written intent, intents or Fragments that are
    /// missing, outside their directories, or reached through a symbolic
    /// link, local Skills without a regular `skills/<name>/SKILL.md`, or an
    /// unsupported lock format.
    pub fn load(root: &Path) -> Result<Self, CollectionError> {
        let manifest_path = root.join(MANIFEST_FILE);
        let text = fs::read_to_string(&manifest_path).map_err(|source| CollectionError::Read {
            path: manifest_path.clone(),
            source,
        })?;
        let manifest = parse(&manifest_path, &text, Manifest::from_toml)?;

        let mut problems = manifest.problems();
        problems.extend(missing_files(root, &manifest)?);
        if !problems.is_empty() {
            return Err(CollectionError::Invalid {
                path: manifest_path,
                problems,
            });
        }

        let lock = load_lock(&root.join(LOCK_FILE))?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest,
            lock,
        })
    }

    /// Collection root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validated Manifest.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Lock; empty when the Collection has no `lock.toml` yet.
    #[must_use]
    pub fn lock(&self) -> &Lock {
        &self.lock
    }

    /// Directory holding the accepted materialised Skill `name`.
    #[must_use]
    pub fn skill_dir(&self, name: &Name) -> PathBuf {
        self.root.join(SKILLS_DIR).join(name.as_str())
    }

    /// The Manifest's Destinations as they apply on this machine.
    ///
    /// `machine_config` is the path of `machine.toml`; a missing file means
    /// no machine-local settings. `home` replaces a leading `~` in paths, so
    /// every resolved path is absolute.
    ///
    /// No resolved path may be, lie inside, or contain the Collection root,
    /// compared both with its canonical path and with its absolute path as
    /// given, so that `install` never writes over the Collection.
    ///
    /// # Errors
    ///
    /// Returns a [`CollectionError`] when the Collection root cannot be
    /// resolved, when `machine.toml` exists but cannot be read or parsed, or
    /// when the result does not fit the Manifest or overlaps the Collection
    /// (see [`MachineConfig::resolve`]).
    pub fn destinations(
        &self,
        machine_config: &Path,
        home: &HomeDir,
    ) -> Result<Vec<ResolvedDestination>, CollectionError> {
        let machine = match read_optional(machine_config)? {
            Some(text) => parse(machine_config, &text, MachineConfig::from_toml)?,
            None => MachineConfig::default(),
        };
        machine
            .resolve(&self.manifest, home, &self.root_forms()?)
            .map_err(|problems| CollectionError::Invalid {
                path: machine_config.to_path_buf(),
                problems,
            })
    }

    /// Absolute forms of the Collection root: canonical (symbolic links
    /// resolved) and, when it differs and has no `..` component, as given.
    fn root_forms(&self) -> Result<Vec<PathBuf>, CollectionError> {
        let read_error = |source| CollectionError::Read {
            path: self.root.clone(),
            source,
        };
        let mut forms = vec![fs::canonicalize(&self.root).map_err(read_error)?];
        let given = std::path::absolute(&self.root).map_err(read_error)?;
        if !given.components().any(|c| c == Component::ParentDir) && !forms.contains(&given) {
            forms.push(given);
        }
        Ok(forms)
    }

    /// Hash of the accepted materialised Skill directory `skills/<name>/`.
    ///
    /// # Errors
    ///
    /// Returns a [`CollectionError`] when the directory cannot be read, is
    /// reached through a symbolic link, or holds a symbolic link, special
    /// file, or non-UTF-8 name.
    pub fn output_digest(&self, name: &Name) -> Result<Digest, CollectionError> {
        let dir = self.skill_dir(name);
        if let Entry::Symlink(link) = inspect(&self.root, &[SKILLS_DIR, name.as_str()])? {
            return Err(HashError::Symlink { path: link }.into());
        }
        Ok(hash::tree_digest(&dir)?)
    }

    /// Hash of the normalised inputs of Skill `name`: its declaration as
    /// written, the contents of the presets it references (in order, not
    /// their names), and the bytes of its written intent. Returns `None` for
    /// local Skills, which have no inputs.
    ///
    /// # Errors
    ///
    /// Returns a [`CollectionError`] when the Skill is not declared or its
    /// intent cannot be read as a regular file of the Collection.
    pub fn inputs_digest(&self, name: &Name) -> Result<Option<Digest>, CollectionError> {
        let skill = self
            .manifest
            .skills
            .get(name)
            .ok_or_else(|| CollectionError::UnknownSkill { name: name.clone() })?;
        let manifest_path = || self.root.join(MANIFEST_FILE);
        let presets = match skill {
            Skill::Local(_) => return Ok(None),
            Skill::Adapt(adapt) => adapt
                .presets
                .iter()
                .filter_map(|preset| self.manifest.presets.get(preset))
                .collect(),
            Skill::Copy(_) | Skill::Composition(_) => Vec::new(),
        };
        let mut declaration = toml::Value::try_from(Inputs { skill, presets }).map_err(|err| {
            CollectionError::Invalid {
                path: manifest_path(),
                problems: vec![Problem::new(
                    format!("skills.{name}"),
                    format!("cannot normalise the declaration: {err}"),
                )],
            }
        })?;
        // Preset names are labels: renaming a preset changes no rule.
        if let Some(toml::Value::Table(declared)) = declaration.get_mut("skill") {
            declared.remove("presets");
        }
        let intent = match skill.intent() {
            Some(intent) => {
                let location = format!("skills.{name}.intent");
                if let Some(problem) =
                    regular_file_problem(&self.root, intent, &location, "written intent")?
                {
                    return Err(CollectionError::Invalid {
                        path: manifest_path(),
                        problems: vec![problem],
                    });
                }
                let path = intent.join_to(&self.root);
                Some(fs::read(&path).map_err(|source| CollectionError::Read { path, source })?)
            }
            None => None,
        };
        Ok(Some(hash::inputs_digest(&declaration, intent.as_deref())))
    }
}

/// Normalised inputs of a Skill, as hashed by [`Collection::inputs_digest`].
#[derive(Serialize)]
struct Inputs<'a> {
    skill: &'a Skill,
    presets: Vec<&'a Preset>,
}

/// Reads `lock.toml`, or returns an empty lock when the file is absent.
///
/// The declared format is checked before the rest of the file, so a lock
/// written by a newer release is reported as such rather than as a schema
/// error.
fn load_lock(path: &Path) -> Result<Lock, CollectionError> {
    let Some(text) = read_optional(path)? else {
        return Ok(Lock::default());
    };
    if let Some(format) = Lock::declared_format(&text)
        && format != LOCK_FORMAT
    {
        return Err(CollectionError::Invalid {
            path: path.to_path_buf(),
            problems: vec![Problem::new(
                "format",
                format!(
                    "lock format {format} is not supported by this release, which reads format {LOCK_FORMAT}; upgrade skill-curator"
                ),
            )],
        });
    }
    let lock = parse(path, &text, Lock::from_toml)?;
    let problems = lock.problems();
    if problems.is_empty() {
        Ok(lock)
    } else {
        Err(CollectionError::Invalid {
            path: path.to_path_buf(),
            problems,
        })
    }
}

/// What a path inside the Collection names, found without following
/// symbolic links below the Collection root.
enum Entry {
    /// Nothing exists at the path.
    Missing,
    /// The path, or one of its parent directories below the root, is a
    /// symbolic link, given by its full path.
    Symlink(PathBuf),
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// Another kind of entry.
    Other,
}

/// Walks `segments` down from `root`, checking each component with
/// `symlink_metadata`, so that no link inside the Collection is followed.
fn inspect(root: &Path, segments: &[&str]) -> Result<Entry, CollectionError> {
    let mut path = root.to_path_buf();
    let mut entry = Entry::Dir;
    for segment in segments {
        if !matches!(entry, Entry::Dir) {
            return Ok(Entry::Missing);
        }
        path.push(segment);
        let file_type = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata.file_type(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Entry::Missing),
            Err(source) => return Err(CollectionError::Read { path, source }),
        };
        entry = if file_type.is_symlink() {
            return Ok(Entry::Symlink(path));
        } else if file_type.is_dir() {
            Entry::Dir
        } else if file_type.is_file() {
            Entry::File
        } else {
            Entry::Other
        };
    }
    Ok(entry)
}

/// Problem with `relative`, the Collection file the Manifest names at
/// `location`, unless it is a regular file reached without symbolic links.
fn regular_file_problem(
    root: &Path,
    relative: &RelativePath,
    location: &str,
    what: &str,
) -> Result<Option<Problem>, CollectionError> {
    let segments = relative.as_str().split('/').collect::<Vec<_>>();
    let message = match inspect(root, &segments)? {
        Entry::File => return Ok(None),
        Entry::Missing => {
            format!("{what} `{relative}` does not exist; create it in the Collection")
        }
        Entry::Symlink(link) => format!(
            "{what} `{relative}` is reached through the symbolic link {}; replace it with a regular file in the Collection",
            link.display()
        ),
        Entry::Dir | Entry::Other => format!("{what} `{relative}` is not a regular file"),
    };
    Ok(Some(Problem::new(location, message)))
}

/// Files the Manifest references that must be regular files of the
/// Collection: written intents, Fragments, and the `SKILL.md` of local
/// Skills.
fn missing_files(root: &Path, manifest: &Manifest) -> Result<Vec<Problem>, CollectionError> {
    let mut problems = Vec::new();
    for (name, skill) in &manifest.skills {
        // Paths outside their directory are already reported by
        // `Manifest::problems`.
        if let Some(intent) = skill.intent()
            && intent.starts_with_dir(INTENTS_DIR)
        {
            problems.extend(regular_file_problem(
                root,
                intent,
                &format!("skills.{name}.intent"),
                "written intent",
            )?);
        }
        if matches!(skill, Skill::Local(_)) {
            let skill_file = format!("{SKILLS_DIR}/{name}/{SKILL_FILE}");
            let message = match inspect(root, &[SKILLS_DIR, name.as_str(), SKILL_FILE])? {
                Entry::File => continue,
                Entry::Missing | Entry::Dir | Entry::Other => {
                    format!("local Skill has no `{skill_file}`; author it in the Collection")
                }
                Entry::Symlink(link) => format!(
                    "local Skill `{skill_file}` is reached through the symbolic link {}; keep the Skill as regular files in the Collection",
                    link.display()
                ),
            };
            problems.push(Problem::new(format!("skills.{name}"), message));
        }
    }
    for (name, destination) in &manifest.destinations {
        let Some(instructions) = &destination.instructions else {
            continue;
        };
        for fragment in &instructions.fragments {
            if fragment.starts_with_dir(FRAGMENTS_DIR) {
                problems.extend(regular_file_problem(
                    root,
                    fragment,
                    &format!("destinations.{name}.instructions.fragments"),
                    "Fragment",
                )?);
            }
        }
    }
    Ok(problems)
}
