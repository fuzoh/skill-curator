//! Deterministic Adaptation: the D2 file rules and D1 frontmatter rules that
//! turn an upstream Skill at a Revision into its post-deterministic view,
//! `det(R)`.
//!
//! `det(R)` applies D2 first ([`files`]), then D1 ([`frontmatter`]) to the
//! `SKILL.md` that remains. Skills are handled as in-memory [`SkillTree`]s
//! read from, and written to, directories that hold regular files only:
//! symbolic links, special files, non-UTF-8 names, and paths with a `..` or
//! `.git` component are refused.

pub mod files;
pub mod frontmatter;

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crate::collection::SKILL_FILE;
use crate::collection::manifest::{CompositionInput, FileRules, FrontmatterRules, Manifest, Skill};
use crate::safety::{PathError, RelativePath};
use files::{FileFilter, FilesError};
use frontmatter::FrontmatterError;

/// Permission bits of a written executable file, before the umask.
const EXECUTABLE_MODE: u32 = 0o755;
/// Permission bits of a written regular file, before the umask.
const REGULAR_MODE: u32 = 0o644;

/// Failure to read, adapt, or write a Skill.
#[derive(Debug)]
#[non_exhaustive]
pub enum AdaptationError {
    /// A file or directory could not be read.
    Read {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// A file or directory could not be written.
    Write {
        /// Path that could not be written.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The Skill directory is, or holds, a symbolic link.
    Symlink {
        /// Path of the link.
        path: PathBuf,
    },
    /// The Skill directory holds something other than regular files and
    /// directories.
    SpecialFile {
        /// Path of the entry.
        path: PathBuf,
    },
    /// The Skill directory is not a directory.
    NotADirectory {
        /// Path given as the Skill directory.
        path: PathBuf,
    },
    /// A name in the Skill directory is not valid UTF-8.
    NonUtf8Name {
        /// Path of the entry.
        path: PathBuf,
    },
    /// A path in the Skill directory cannot be kept safely, such as one with
    /// a `.git` component.
    UnsafePath {
        /// Path relative to the Skill directory.
        path: String,
        /// Why it is refused.
        reason: PathError,
    },
    /// `SKILL.md` is not valid UTF-8.
    SkillFileNotUtf8,
    /// D2 file rules cannot be applied.
    Files(FilesError),
    /// D1 frontmatter rules cannot be applied.
    Frontmatter(FrontmatterError),
}

impl fmt::Display for AdaptationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Write { path, source } => write!(f, "cannot write {}: {source}", path.display()),
            Self::Symlink { path } => write!(
                f,
                "{} is a symbolic link; Skills hold regular files only",
                path.display()
            ),
            Self::SpecialFile { path } => write!(
                f,
                "{} is neither a regular file nor a directory",
                path.display()
            ),
            Self::NotADirectory { path } => write!(f, "{} is not a directory", path.display()),
            Self::NonUtf8Name { path } => {
                write!(f, "{} has a name that is not valid UTF-8", path.display())
            }
            Self::UnsafePath { path, reason } => {
                write!(f, "the Skill file `{path}` cannot be kept: {reason}")
            }
            Self::SkillFileNotUtf8 => write!(f, "`{SKILL_FILE}` is not valid UTF-8"),
            Self::Files(err) => err.fmt(f),
            Self::Frontmatter(err) => write!(f, "`{SKILL_FILE}`: {err}"),
        }
    }
}

impl std::error::Error for AdaptationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Files(err) => Some(err),
            Self::Frontmatter(err) => Some(err),
            _ => None,
        }
    }
}

impl From<FilesError> for AdaptationError {
    fn from(err: FilesError) -> Self {
        Self::Files(err)
    }
}

impl From<FrontmatterError> for AdaptationError {
    fn from(err: FrontmatterError) -> Self {
        Self::Frontmatter(err)
    }
}

/// One file of a Skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    /// File contents.
    pub contents: Vec<u8>,
    /// Whether the file is executable.
    pub executable: bool,
}

/// The files of a Skill directory, by path relative to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillTree {
    files: BTreeMap<RelativePath, SkillFile>,
}

impl SkillTree {
    /// Reads every file below `dir`.
    ///
    /// # Errors
    ///
    /// Returns an [`AdaptationError`] when `dir` is not a directory, is a
    /// symbolic link, or holds a symbolic link, a special file, a non-UTF-8
    /// name, or a `.git` entry, or when a file cannot be read.
    pub fn read(dir: &Path) -> Result<Self, AdaptationError> {
        let root_type = fs::symlink_metadata(dir)
            .map_err(|source| AdaptationError::Read {
                path: dir.to_path_buf(),
                source,
            })?
            .file_type();
        if root_type.is_symlink() {
            return Err(AdaptationError::Symlink {
                path: dir.to_path_buf(),
            });
        }
        if !root_type.is_dir() {
            return Err(AdaptationError::NotADirectory {
                path: dir.to_path_buf(),
            });
        }
        let mut files = BTreeMap::new();
        let mut pending = vec![(dir.to_path_buf(), String::new())];
        while let Some((current, prefix)) = pending.pop() {
            let read_error = |source| AdaptationError::Read {
                path: current.clone(),
                source,
            };
            for entry in fs::read_dir(&current).map_err(read_error)? {
                let entry = entry.map_err(read_error)?;
                let path = entry.path();
                let Ok(name) = entry.file_name().into_string() else {
                    return Err(AdaptationError::NonUtf8Name { path });
                };
                let relative = format!("{prefix}{name}");
                let checked = RelativePath::exact(&relative).map_err(|reason| {
                    AdaptationError::UnsafePath {
                        path: relative.clone(),
                        reason,
                    }
                })?;
                let metadata =
                    fs::symlink_metadata(&path).map_err(|source| AdaptationError::Read {
                        path: path.clone(),
                        source,
                    })?;
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    return Err(AdaptationError::Symlink { path });
                } else if file_type.is_dir() {
                    pending.push((path, format!("{relative}/")));
                } else if file_type.is_file() {
                    let contents =
                        fs::read(&path).map_err(|source| AdaptationError::Read { path, source })?;
                    let executable = metadata.permissions().mode() & 0o111 != 0;
                    files.insert(
                        checked,
                        SkillFile {
                            contents,
                            executable,
                        },
                    );
                } else {
                    return Err(AdaptationError::SpecialFile { path });
                }
            }
        }
        Ok(Self { files })
    }

    /// Paths of the files, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &RelativePath> {
        self.files.keys()
    }

    /// The file at `path`.
    #[must_use]
    pub fn get(&self, path: &RelativePath) -> Option<&SkillFile> {
        self.files.get(path)
    }

    /// Adds or replaces the file at `path`.
    pub fn insert(&mut self, path: RelativePath, file: SkillFile) {
        self.files.insert(path, file);
    }

    /// Removes the file at `path`, returning it.
    pub fn remove(&mut self, path: &RelativePath) -> Option<SkillFile> {
        self.files.remove(path)
    }

    /// The text of the top-level `SKILL.md`.
    ///
    /// # Errors
    ///
    /// Returns [`FilesError::MissingSkillFile`] (wrapped) when there is none,
    /// or [`AdaptationError::SkillFileNotUtf8`].
    pub fn skill_md(&self) -> Result<&str, AdaptationError> {
        let file = self
            .files
            .get(&skill_file_path())
            .ok_or(AdaptationError::Files(FilesError::MissingSkillFile))?;
        std::str::from_utf8(&file.contents).map_err(|_| AdaptationError::SkillFileNotUtf8)
    }

    /// Replaces the text of the top-level `SKILL.md`, keeping its mode.
    pub fn set_skill_md(&mut self, text: String) {
        let path = skill_file_path();
        let executable = self.files.get(&path).is_some_and(|file| file.executable);
        self.files.insert(
            path,
            SkillFile {
                contents: text.into_bytes(),
                executable,
            },
        );
    }

    /// The files `filter` keeps, which must include `SKILL.md`.
    ///
    /// # Errors
    ///
    /// Returns [`AdaptationError::Files`] when the tree has no `SKILL.md` or
    /// the filter drops it.
    pub fn filtered(&self, filter: &FileFilter) -> Result<Self, AdaptationError> {
        let kept = filter.select(self.files.keys())?;
        Ok(Self {
            files: self
                .files
                .iter()
                .filter(|(path, _)| kept.contains(*path))
                .map(|(path, file)| (path.clone(), file.clone()))
                .collect(),
        })
    }

    /// Writes the tree into `into`, which must not exist yet; its parent
    /// must. Files are created, never opened through an existing path, with
    /// mode `0644`, or `0755` when executable, before the umask.
    ///
    /// # Errors
    ///
    /// Returns [`AdaptationError::Write`] when `into` exists or a file
    /// cannot be written.
    pub fn write_new(&self, into: &Path) -> Result<(), AdaptationError> {
        let write_error = |path: &Path| {
            let path = path.to_path_buf();
            move |source| AdaptationError::Write { path, source }
        };
        fs::create_dir(into).map_err(write_error(into))?;
        for (relative, file) in &self.files {
            let path = relative.join_to(into);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(write_error(parent))?;
            }
            let mode = if file.executable {
                EXECUTABLE_MODE
            } else {
                REGULAR_MODE
            };
            let mut out = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(&path)
                .map_err(write_error(&path))?;
            out.write_all(&file.contents).map_err(write_error(&path))?;
        }
        Ok(())
    }
}

fn skill_file_path() -> RelativePath {
    RelativePath::skill_file()
}

/// The deterministic rules of a Skill or Composition input, presets first,
/// in application order. Empty rule sets are left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rules<'a> {
    /// D2 file rule sets.
    pub files: Vec<&'a FileRules>,
    /// D1 frontmatter rule sets.
    pub frontmatter: Vec<&'a FrontmatterRules>,
}

impl<'a> Rules<'a> {
    /// Rules of `skill`: for an Adapt Skill, its presets in order, then its
    /// own rules; for a Composition, its D1 rules (its inputs' D2 rules are
    /// given by [`Rules::of_input`]); none for Copy and local Skills.
    #[must_use]
    pub fn of(manifest: &'a Manifest, skill: &'a Skill) -> Self {
        let mut rules = Self::default();
        match skill {
            Skill::Adapt(adapt) => {
                for preset in adapt
                    .presets
                    .iter()
                    .filter_map(|name| manifest.presets.get(name))
                {
                    rules.push(Some(&preset.files), Some(&preset.frontmatter));
                }
                rules.push(Some(&adapt.files), Some(&adapt.frontmatter));
            }
            Skill::Composition(composition) => rules.push(None, Some(&composition.frontmatter)),
            Skill::Copy(_) | Skill::Local(_) => {}
        }
        rules
    }

    /// D2 rules of one Composition input.
    #[must_use]
    pub fn of_input(input: &'a CompositionInput) -> Self {
        let mut rules = Self::default();
        rules.push(Some(&input.files), None);
        rules
    }

    fn push(&mut self, files: Option<&'a FileRules>, frontmatter: Option<&'a FrontmatterRules>) {
        if let Some(files) = files.filter(|files| !files.is_empty()) {
            self.files.push(files);
        }
        if let Some(frontmatter) = frontmatter.filter(|rules| !rules.is_empty()) {
            self.frontmatter.push(frontmatter);
        }
    }
}

/// `det(R)`: applies D2 then D1 to `upstream`, the Skill extracted at a
/// Revision.
///
/// # Errors
///
/// Returns an [`AdaptationError`] when a glob is invalid, the D2 rules drop
/// `SKILL.md` or the Skill has none, or its frontmatter cannot be read or
/// edited.
pub fn det(upstream: &SkillTree, rules: &Rules<'_>) -> Result<SkillTree, AdaptationError> {
    let mut tree = upstream.filtered(&FileFilter::new(&rules.files)?)?;
    let text = frontmatter::apply_rules(tree.skill_md()?, &rules.frontmatter)?;
    tree.set_skill_md(text);
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;
    use crate::collection::manifest::Manifest;
    use crate::safety::PathPattern;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn upstream() -> TempDir {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "SKILL.md",
            "---\nname: grill-me\nallowed-tools: Read\n---\nBody\n---\n```!\nx\n```\n",
        );
        write(dir.path(), "evals/case.md", "eval");
        write(dir.path(), "references/guide.md", "guide");
        write(dir.path(), "scripts/run.sh", "#!/bin/sh\n");
        fs::set_permissions(
            dir.path().join("scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        dir
    }

    fn manifest(text: &str) -> Manifest {
        Manifest::from_toml(text).unwrap()
    }

    #[test]
    fn reads_and_writes_regular_files_with_their_mode() {
        let dir = upstream();
        let tree = SkillTree::read(dir.path()).unwrap();
        assert_eq!(
            tree.paths().map(RelativePath::as_str).collect::<Vec<_>>(),
            [
                "SKILL.md",
                "evals/case.md",
                "references/guide.md",
                "scripts/run.sh"
            ]
        );
        let script = tree
            .get(&RelativePath::exact("scripts/run.sh").unwrap())
            .unwrap();
        assert!(script.executable);

        let out = TempDir::new().unwrap();
        let into = out.path().join("copy");
        tree.write_new(&into).unwrap();
        assert_eq!(SkillTree::read(&into).unwrap(), tree);
        let mode = fs::metadata(into.join("references/guide.md"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0);
        assert!(matches!(
            tree.write_new(&into),
            Err(AdaptationError::Write { .. })
        ));
    }

    #[test]
    fn symlinks_and_special_paths_are_refused() {
        let dir = upstream();
        symlink("/etc/passwd", dir.path().join("references/link")).unwrap();
        assert!(matches!(
            SkillTree::read(dir.path()),
            Err(AdaptationError::Symlink { .. })
        ));

        let dir = upstream();
        symlink(dir.path(), dir.path().join("loop")).unwrap();
        assert!(matches!(
            SkillTree::read(dir.path()),
            Err(AdaptationError::Symlink { .. })
        ));

        let dir = upstream();
        let link = TempDir::new().unwrap();
        let linked = link.path().join("skill");
        symlink(dir.path(), &linked).unwrap();
        assert!(matches!(
            SkillTree::read(&linked),
            Err(AdaptationError::Symlink { .. })
        ));

        let dir = upstream();
        write(dir.path(), ".git/config", "[core]");
        assert!(matches!(
            SkillTree::read(dir.path()),
            Err(AdaptationError::UnsafePath {
                reason: PathError::GitDir,
                ..
            })
        ));

        let dir = upstream();
        assert!(matches!(
            SkillTree::read(&dir.path().join("SKILL.md")),
            Err(AdaptationError::NotADirectory { .. })
        ));
    }

    #[test]
    fn det_applies_file_rules_then_frontmatter_rules() {
        let manifest = manifest(
            r#"
[sources.up]
url = "https://example.com/up.git"

[presets.no-evals]
files = { exclude = ["evals"] }
frontmatter = { remove = ["allowed-tools"] }

[skills.grill]
mode = "adapt"
source = "up"
upstream_path = "."
presets = ["no-evals"]
files = { exclude = ["scripts/**"] }
frontmatter = { set = { license = "MIT" } }
"#,
        );
        let skill = &manifest.skills[&crate::safety::Name::new("grill").unwrap()];
        let rules = Rules::of(&manifest, skill);
        assert_eq!(rules.files.len(), 2);
        assert_eq!(rules.frontmatter.len(), 2);

        let dir = upstream();
        let tree = det(&SkillTree::read(dir.path()).unwrap(), &rules).unwrap();
        assert_eq!(
            tree.paths().map(RelativePath::as_str).collect::<Vec<_>>(),
            ["SKILL.md", "references/guide.md"]
        );
        assert_eq!(
            tree.skill_md().unwrap(),
            "---\nname: grill-me\nlicense: MIT\n---\nBody\n---\n```!\nx\n```\n"
        );
    }

    #[test]
    fn det_of_a_copy_is_the_upstream_skill() {
        let dir = upstream();
        let upstream = SkillTree::read(dir.path()).unwrap();
        assert_eq!(det(&upstream, &Rules::default()).unwrap(), upstream);
    }

    #[test]
    fn det_without_frontmatter_rules_keeps_a_frontmatter_d1_cannot_edit() {
        let dir = upstream();
        let lax = "---\nname: grill-me\ndescription: Use when: the user asks\n---\nBody\n";
        write(dir.path(), "SKILL.md", lax);
        let upstream = SkillTree::read(dir.path()).unwrap();
        assert_eq!(det(&upstream, &Rules::default()).unwrap(), upstream);
        let exclude = FileRules {
            exclude: vec![PathPattern::new("evals").unwrap()],
            include: None,
        };
        let rules = Rules {
            files: vec![&exclude],
            frontmatter: Vec::new(),
        };
        assert_eq!(det(&upstream, &rules).unwrap().skill_md().unwrap(), lax);
    }

    #[test]
    fn det_refuses_rules_that_drop_skill_md() {
        let dir = upstream();
        let exclude = FileRules {
            exclude: vec![PathPattern::new("*.md").unwrap()],
            include: None,
        };
        let rules = Rules {
            files: vec![&exclude],
            frontmatter: Vec::new(),
        };
        assert!(matches!(
            det(&SkillTree::read(dir.path()).unwrap(), &rules),
            Err(AdaptationError::Files(FilesError::SkillFileDropped { .. }))
        ));
    }

    #[test]
    fn composition_rules_split_d1_and_per_input_d2() {
        let manifest = manifest(
            r#"
[sources.up]
url = "https://example.com/up.git"

[skills.review]
mode = "composition"
intent = "intents/review.md"
frontmatter = { keep_only = ["name", "description"] }

[skills.review.inputs.a]
source = "up"
upstream_path = "a"
files = { exclude = ["evals"] }

[skills.review.inputs.b]
source = "up"
upstream_path = "b"
"#,
        );
        let skill = &manifest.skills[&crate::safety::Name::new("review").unwrap()];
        let rules = Rules::of(&manifest, skill);
        assert!(rules.files.is_empty());
        assert_eq!(rules.frontmatter.len(), 1);
        let Skill::Composition(composition) = skill else {
            panic!("not a Composition");
        };
        let inputs = composition
            .inputs
            .values()
            .map(|input| Rules::of_input(input).files.len())
            .collect::<Vec<_>>();
        assert_eq!(inputs, [1, 0]);
    }
}
