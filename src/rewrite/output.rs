//! Validation of the proposed Skill an agent wrote into `output/`.
//!
//! `output/` must be a real directory holding regular files and directories
//! only: a symbolic link (anywhere, including `output/` itself), a hard link
//! to a file elsewhere, a special file, a non-UTF-8 name, or a `.git`
//! component is refused, so nothing outside `output/` can reach the proposed
//! Skill. `SKILL.md` must exist and start with a valid YAML frontmatter, on
//! which the D1 rules are then enforced again.

use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use super::staging::OUTPUT_DIR;
use crate::adaptation::frontmatter::{FrontmatterError, SkillDocument, apply_rules};
use crate::adaptation::{AdaptationError, SkillTree};
use crate::collection::SKILL_FILE;
use crate::collection::manifest::FrontmatterRules;
use crate::safety::{PathError, RelativePath};

/// Why the proposed Skill in `output/` is rejected.
#[derive(Debug)]
#[non_exhaustive]
pub enum OutputError {
    /// A file or directory could not be read.
    Read {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// `output/` itself is a symbolic link or not a directory.
    NotADirectory,
    /// An entry is a symbolic link.
    Symlink {
        /// Path relative to `output/`.
        path: String,
    },
    /// A file has other hard links, so its contents may live outside
    /// `output/`.
    HardLink {
        /// Path relative to `output/`.
        path: String,
    },
    /// An entry is neither a regular file nor a directory.
    SpecialFile {
        /// Path relative to `output/`.
        path: String,
    },
    /// A name is not valid UTF-8.
    NonUtf8Name {
        /// Path of the entry.
        path: PathBuf,
    },
    /// A path cannot be kept in a Skill, such as one with a `.git`
    /// component.
    UnsafePath {
        /// Path relative to `output/`.
        path: String,
        /// Why it is refused.
        reason: PathError,
    },
    /// The files could not be read as a Skill.
    Skill(AdaptationError),
    /// There is no top-level `SKILL.md`.
    MissingSkillFile,
    /// `SKILL.md` is not valid UTF-8.
    SkillFileNotUtf8,
    /// `SKILL.md` does not start with a frontmatter block.
    NoFrontmatter,
    /// The frontmatter of `SKILL.md` is not valid.
    InvalidFrontmatter(FrontmatterError),
    /// The D1 rules cannot be enforced on the frontmatter.
    Rules(FrontmatterError),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotADirectory => write!(
                f,
                "`{OUTPUT_DIR}/` must be a directory, not a symbolic link or another kind of file"
            ),
            Self::Symlink { path } => write!(
                f,
                "`{OUTPUT_DIR}/{path}` is a symbolic link; a Skill holds regular files only"
            ),
            Self::HardLink { path } => write!(
                f,
                "`{OUTPUT_DIR}/{path}` is a hard link to a file that may lie outside `{OUTPUT_DIR}/`; write a copy instead"
            ),
            Self::SpecialFile { path } => write!(
                f,
                "`{OUTPUT_DIR}/{path}` is neither a regular file nor a directory"
            ),
            Self::NonUtf8Name { path } => {
                write!(f, "{} has a name that is not valid UTF-8", path.display())
            }
            Self::UnsafePath { path, reason } => {
                write!(f, "`{OUTPUT_DIR}/{path}` cannot be kept: {reason}")
            }
            Self::Skill(err) => write!(f, "`{OUTPUT_DIR}/`: {err}"),
            Self::MissingSkillFile => write!(f, "`{OUTPUT_DIR}/{SKILL_FILE}` is missing"),
            Self::SkillFileNotUtf8 => {
                write!(f, "`{OUTPUT_DIR}/{SKILL_FILE}` is not valid UTF-8")
            }
            Self::NoFrontmatter => write!(
                f,
                "`{OUTPUT_DIR}/{SKILL_FILE}` does not start with a YAML frontmatter block between `---` lines"
            ),
            Self::InvalidFrontmatter(err) => write!(f, "`{OUTPUT_DIR}/{SKILL_FILE}`: {err}"),
            Self::Rules(err) => write!(
                f,
                "the frontmatter rules (D1) cannot be enforced on `{OUTPUT_DIR}/{SKILL_FILE}`: {err}"
            ),
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Skill(err) => Some(err),
            Self::InvalidFrontmatter(err) | Self::Rules(err) => Some(err),
            _ => None,
        }
    }
}

/// Validates the proposed Skill in `output` and enforces `frontmatter`, the
/// D1 rule sets, on its `SKILL.md`. Returns the Skill and whether enforcing
/// the rules changed `SKILL.md`.
///
/// # Errors
///
/// Returns an [`OutputError`] when `output` is rejected.
pub(super) fn validate(
    output: &Path,
    frontmatter: &[FrontmatterRules],
) -> Result<(SkillTree, bool), OutputError> {
    check_entries(output)?;
    // The checks above refused what this pass refuses, unless `output/`
    // changed in between.
    let mut tree = SkillTree::read(output).map_err(OutputError::Skill)?;
    let file = tree
        .get(&RelativePath::skill_file())
        .ok_or(OutputError::MissingSkillFile)?;
    let text = std::str::from_utf8(&file.contents).map_err(|_| OutputError::SkillFileNotUtf8)?;
    let document = SkillDocument::parse(text).map_err(OutputError::InvalidFrontmatter)?;
    if !document.has_frontmatter() {
        return Err(OutputError::NoFrontmatter);
    }
    let rules = frontmatter.iter().collect::<Vec<_>>();
    let enforced = apply_rules(text, &rules).map_err(OutputError::Rules)?;
    let changed = enforced != text;
    if changed {
        tree.set_skill_md(enforced);
    }
    Ok((tree, changed))
}

/// Walks `output` without following symbolic links and refuses every entry
/// a Skill cannot hold.
fn check_entries(output: &Path) -> Result<(), OutputError> {
    let root = fs::symlink_metadata(output).map_err(|source| OutputError::Read {
        path: output.to_path_buf(),
        source,
    })?;
    if !root.file_type().is_dir() {
        return Err(OutputError::NotADirectory);
    }
    let mut pending = vec![(output.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        let read_error = |source| OutputError::Read {
            path: dir.clone(),
            source,
        };
        for entry in fs::read_dir(&dir).map_err(read_error)? {
            let entry = entry.map_err(read_error)?;
            let path = entry.path();
            let Ok(name) = entry.file_name().into_string() else {
                return Err(OutputError::NonUtf8Name { path });
            };
            let relative = format!("{prefix}{name}");
            if let Err(reason) = RelativePath::exact(&relative) {
                return Err(OutputError::UnsafePath {
                    path: relative,
                    reason,
                });
            }
            let metadata = fs::symlink_metadata(&path).map_err(|source| OutputError::Read {
                path: path.clone(),
                source,
            })?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                return Err(OutputError::Symlink { path: relative });
            } else if kind.is_dir() {
                pending.push((path, format!("{relative}/")));
            } else if !kind.is_file() {
                return Err(OutputError::SpecialFile { path: relative });
            } else if metadata.nlink() > 1 {
                return Err(OutputError::HardLink { path: relative });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    const SKILL_MD: &str = "---\nname: grill\ndescription: Grill.\n---\nBody\n";

    fn output(skill_md: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();
        if let Some(text) = skill_md {
            fs::write(dir.path().join(SKILL_FILE), text).unwrap();
        }
        fs::create_dir(dir.path().join("references")).unwrap();
        fs::write(dir.path().join("references/guide.md"), "guide").unwrap();
        dir
    }

    fn rules(toml: &str) -> FrontmatterRules {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn accepts_a_valid_skill_without_rules() {
        let dir = output(Some(SKILL_MD));
        let (tree, changed) = validate(dir.path(), &[]).unwrap();
        assert!(!changed);
        assert_eq!(tree.skill_md().unwrap(), SKILL_MD);
        assert_eq!(tree.paths().count(), 2);
    }

    #[test]
    fn enforces_the_rules_in_order() {
        let dir = output(Some(
            "---\nname: renamed\ndescription: Grill.\nicon: x\nextra: y\n---\nBody\n",
        ));
        let sets = [
            rules("remove = [\"icon\"]"),
            rules("keep_only = [\"name\", \"description\"]\nset = { name = \"grill\" }"),
        ];
        let (tree, changed) = validate(dir.path(), &sets).unwrap();
        assert!(changed);
        assert_eq!(tree.skill_md().unwrap(), SKILL_MD);
    }

    #[test]
    fn refuses_a_missing_or_invalid_skill_md() {
        let dir = output(None);
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::MissingSkillFile)
        ));
        let dir = output(Some("# No frontmatter\n"));
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::NoFrontmatter)
        ));
        let dir = output(Some("---\nname: [unclosed\n---\nBody\n"));
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::InvalidFrontmatter(
                FrontmatterError::Syntax { .. }
            ))
        ));
        let dir = output(Some("---\nname: grill\n"));
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::InvalidFrontmatter(FrontmatterError::Unclosed))
        ));
        let dir = output(Some("---\n- a\n- b\n---\n"));
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::InvalidFrontmatter(
                FrontmatterError::NotAMapping
            ))
        ));
        let dir = output(None);
        fs::write(dir.path().join(SKILL_FILE), [0xff, 0xfe]).unwrap();
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::SkillFileNotUtf8)
        ));
        let dir = output(None);
        fs::write(dir.path().join("skill.md"), SKILL_MD).unwrap();
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::MissingSkillFile)
        ));
    }

    #[test]
    fn refuses_rules_that_cannot_be_enforced() {
        // Setting `name` would orphan the alias that `description` uses.
        let dir = output(Some("---\nname: &n grill\ndescription: *n\n---\nBody\n"));
        assert!(matches!(
            validate(dir.path(), &[rules("set = { name = \"other\" }")]),
            Err(OutputError::Rules(FrontmatterError::UnsafeEdit))
        ));
        // A merge key could bring back keys the rules remove; it is refused
        // even without rules.
        let dir = output(Some("---\nbase: &b {name: x}\n<<: *b\n---\n"));
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::InvalidFrontmatter(
                FrontmatterError::MergeKey { .. }
            ))
        ));
    }

    #[test]
    fn refuses_links_special_files_and_git_directories() {
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("secret");
        fs::write(&secret, "secret").unwrap();

        let dir = output(Some(SKILL_MD));
        symlink(&secret, dir.path().join("references/secret")).unwrap();
        let err = validate(dir.path(), &[]).unwrap_err();
        assert!(
            matches!(&err, OutputError::Symlink { path } if path == "references/secret"),
            "{err}"
        );

        let dir = output(Some(SKILL_MD));
        symlink("../../..", dir.path().join("up")).unwrap();
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::Symlink { .. })
        ));

        let dir = output(Some(SKILL_MD));
        fs::hard_link(&secret, dir.path().join("references/copy")).unwrap();
        let err = validate(dir.path(), &[]).unwrap_err();
        assert!(
            matches!(&err, OutputError::HardLink { path } if path == "references/copy"),
            "{err}"
        );

        let dir = output(Some(SKILL_MD));
        fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::UnsafePath {
                reason: PathError::GitDir,
                ..
            })
        ));

        let dir = output(Some(SKILL_MD));
        let fifo = dir.path().join("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            validate(dir.path(), &[]),
            Err(OutputError::SpecialFile { .. })
        ));

        let real = output(Some(SKILL_MD));
        let links = TempDir::new().unwrap();
        let linked = links.path().join("output");
        symlink(real.path(), &linked).unwrap();
        assert!(matches!(
            validate(&linked, &[]),
            Err(OutputError::NotADirectory)
        ));
        fs::write(links.path().join("file"), "x").unwrap();
        assert!(matches!(
            validate(&links.path().join("file"), &[]),
            Err(OutputError::NotADirectory)
        ));
    }
}
