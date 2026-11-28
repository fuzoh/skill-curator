//! The temporary staging directory of one rewrite: the brief, read-only
//! reference views, and the writable `output/` directory.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::brief::{self, Prefill, UpstreamChange};
use super::{Inputs, Job, Proposal, RewriteError, output};
use crate::adaptation::SkillTree;
use crate::collection::manifest::FrontmatterRules;
use crate::diff::TreeDiff;
use crate::safety::Name;

/// Instructions for the agent.
pub const BRIEF_FILE: &str = "BRIEF.md";
/// `det(R_old)` of an Adaptation.
pub const BASE_DIR: &str = "base";
/// `det(R_new)` of an Adaptation.
pub const UPSTREAM_DIR: &str = "upstream";
/// `accepted_local`.
pub const CURRENT_DIR: &str = "current";
/// Post-D2 views of the Composition inputs at the new Revision.
pub const INPUTS_DIR: &str = "inputs";
/// `R_old -> R_new` diffs.
pub const DIFFS_DIR: &str = "diffs";
/// The writable directory receiving the proposed Skill.
pub const OUTPUT_DIR: &str = "output";

/// Prefix of staging directories in the system temporary directory.
const STAGING_PREFIX: &str = "skill-curator-rewrite-";
/// Name of the diff of an Adaptation's upstream Skill, in `diffs/`.
const UPSTREAM_DIFF: &str = "upstream";
/// Mode of read-only files and directories that can be traversed or run.
const READ_ONLY_EXEC_MODE: u32 = 0o555;
/// Mode of read-only files.
const READ_ONLY_MODE: u32 = 0o444;
/// Mode given back to directories before removal.
const WRITABLE_DIR_MODE: u32 = 0o755;

/// A prepared staging directory, removed when dropped.
#[derive(Debug)]
pub struct Staging {
    dir: TempDir,
    skill: Name,
    current: Option<SkillTree>,
    frontmatter: Vec<FrontmatterRules>,
}

impl Staging {
    /// Prepares a new staging directory for `job` in the system temporary
    /// directory: writes `BRIEF.md` and the reference views, makes them
    /// read-only, and pre-populates `output/` with `accepted_local`, else
    /// with `det(R_new)` for an Adaptation, else leaves it empty.
    ///
    /// # Errors
    ///
    /// Returns [`RewriteError::Staging`] or [`RewriteError::View`] when a
    /// file or directory cannot be written.
    pub fn create(job: Job) -> Result<Self, RewriteError> {
        let temp = std::env::temp_dir();
        let dir = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .map_err(staging_error(&temp))?;
        let staging = Self {
            dir,
            skill: job.skill.clone(),
            current: job.current.clone(),
            frontmatter: job.frontmatter.clone(),
        };
        staging.populate(&job)?;
        Ok(staging)
    }

    fn populate(&self, job: &Job) -> Result<(), RewriteError> {
        let root = self.path();
        let mut sealed = Vec::new();
        let mut changes = Vec::new();
        match &job.inputs {
            Inputs::Adaptation { base, upstream } => {
                upstream.write_new(&root.join(UPSTREAM_DIR))?;
                sealed.push(root.join(UPSTREAM_DIR));
                if let Some(base) = base {
                    base.write_new(&root.join(BASE_DIR))?;
                    sealed.push(root.join(BASE_DIR));
                }
                changes.push(UpstreamChange {
                    input: None,
                    diff: base
                        .as_ref()
                        .map(|base| TreeDiff::new(base, upstream, BASE_DIR, UPSTREAM_DIR)),
                });
            }
            Inputs::Composition(inputs) => {
                let inputs_dir = root.join(INPUTS_DIR);
                fs::create_dir(&inputs_dir).map_err(staging_error(&inputs_dir))?;
                sealed.push(inputs_dir.clone());
                for (name, revisions) in inputs {
                    revisions.new.write_new(&inputs_dir.join(name.as_str()))?;
                    changes.push(UpstreamChange {
                        input: Some(name.clone()),
                        diff: revisions
                            .old
                            .as_ref()
                            .map(|old| TreeDiff::new(old, &revisions.new, "old", "new")),
                    });
                }
            }
        }
        let diffs = changes
            .iter()
            .filter_map(|change| {
                let diff = change.diff.as_ref().filter(|diff| !diff.is_empty())?;
                Some((diff_path(change.input.as_ref()), diff))
            })
            .collect::<Vec<_>>();
        if !diffs.is_empty() {
            let diffs_dir = root.join(DIFFS_DIR);
            fs::create_dir(&diffs_dir).map_err(staging_error(&diffs_dir))?;
            for (path, diff) in diffs {
                write_file(&root.join(path), &diff.to_string())?;
            }
            sealed.push(diffs_dir);
        }
        if let Some(current) = &job.current {
            current.write_new(&root.join(CURRENT_DIR))?;
            sealed.push(root.join(CURRENT_DIR));
        }
        let output_dir = self.output_dir();
        let prefill = match (&job.current, &job.inputs) {
            (Some(current), _) => {
                current.write_new(&output_dir)?;
                Prefill::Current
            }
            (None, Inputs::Adaptation { upstream, .. }) => {
                upstream.write_new(&output_dir)?;
                Prefill::Upstream
            }
            (None, Inputs::Composition(_)) => {
                fs::create_dir(&output_dir).map_err(staging_error(&output_dir))?;
                Prefill::Empty
            }
        };
        let brief_path = root.join(BRIEF_FILE);
        write_file(&brief_path, &brief::render(job, &changes, prefill))?;
        sealed.push(brief_path);
        for path in &sealed {
            seal(path)?;
        }
        Ok(())
    }

    /// The staging directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The writable directory receiving the proposed Skill.
    #[must_use]
    pub fn output_dir(&self) -> PathBuf {
        self.path().join(OUTPUT_DIR)
    }

    /// The Skill being rewritten.
    #[must_use]
    pub fn skill(&self) -> &Name {
        &self.skill
    }

    /// Validates the proposed Skill in `output/`, enforces the D1 rules
    /// again on its `SKILL.md`, and diffs it against `accepted_local`.
    ///
    /// # Errors
    ///
    /// Returns [`RewriteError::Output`] when `output/` is rejected (see
    /// [`OutputError`](super::OutputError)).
    pub fn collect(&self) -> Result<Proposal, RewriteError> {
        let (skill, frontmatter_enforced) =
            output::validate(&self.output_dir(), &self.frontmatter)?;
        let empty = SkillTree::default();
        let current = self.current.as_ref().unwrap_or(&empty);
        let diff = TreeDiff::new(current, &skill, CURRENT_DIR, OUTPUT_DIR);
        Ok(Proposal {
            skill,
            diff,
            frontmatter_enforced,
        })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Removing entries needs write permission on their directory: the
        // read-only views, and anything the agent made read-only. Only real
        // directories are touched; symbolic links are never followed.
        fn make_writable(dir: &Path) {
            if fs::set_permissions(dir, fs::Permissions::from_mode(WRITABLE_DIR_MODE)).is_err() {
                return;
            }
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    make_writable(&entry.path());
                }
            }
        }
        make_writable(self.dir.path());
    }
}

/// Path of the diff of Composition input `input`, or of an Adaptation's
/// upstream Skill, relative to the staging directory.
pub(super) fn diff_path(input: Option<&Name>) -> String {
    let name = input.map_or(UPSTREAM_DIFF, Name::as_str);
    format!("{DIFFS_DIR}/{name}.diff")
}

fn staging_error(path: &Path) -> impl FnOnce(std::io::Error) -> RewriteError {
    let path = path.to_path_buf();
    move |source| RewriteError::Staging { path, source }
}

/// Creates a new file holding `text`; never opens an existing path.
fn write_file(path: &Path, text: &str) -> Result<(), RewriteError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .map_err(staging_error(path))?;
    file.write_all(text.as_bytes()).map_err(staging_error(path))
}

/// Makes the file or directory tree at `path` read-only, files first, then
/// each directory after its contents. Executable files stay executable.
/// Symbolic links are never followed (the views hold none).
fn seal(path: &Path) -> Result<(), RewriteError> {
    let metadata = fs::symlink_metadata(path).map_err(staging_error(path))?;
    let kind = metadata.file_type();
    let mode = if kind.is_dir() {
        for entry in fs::read_dir(path).map_err(staging_error(path))? {
            seal(&entry.map_err(staging_error(path))?.path())?;
        }
        READ_ONLY_EXEC_MODE
    } else if kind.is_file() {
        if metadata.permissions().mode() & 0o111 == 0 {
            READ_ONLY_MODE
        } else {
            READ_ONLY_EXEC_MODE
        }
    } else {
        return Ok(());
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(staging_error(path))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::adaptation::SkillFile;
    use crate::rewrite::InputRevisions;
    use crate::safety::RelativePath;

    fn tree(files: &[(&str, &str)]) -> SkillTree {
        let mut tree = SkillTree::default();
        for (path, contents) in files {
            tree.insert(
                RelativePath::exact(path).unwrap(),
                SkillFile {
                    contents: contents.as_bytes().to_vec(),
                    executable: path.ends_with(".sh"),
                },
            );
        }
        tree
    }

    fn job(inputs: Inputs, current: Option<SkillTree>) -> Job {
        Job {
            skill: Name::new("grill").unwrap(),
            inputs,
            current,
            intent: Some("Keep it short.\n".to_owned()),
            frontmatter: Vec::new(),
            guidance: None,
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn views_are_read_only_and_output_is_writable() {
        let upstream = tree(&[("SKILL.md", "---\nname: a\n---\nNew\n"), ("run.sh", "x")]);
        let base = tree(&[("SKILL.md", "---\nname: a\n---\nOld\n")]);
        let staging = Staging::create(job(
            Inputs::Adaptation {
                base: Some(base),
                upstream,
            },
            None,
        ))
        .unwrap();
        let root = staging.path().to_path_buf();
        assert_eq!(mode(&root.join(BRIEF_FILE)), READ_ONLY_MODE);
        assert_eq!(mode(&root.join(UPSTREAM_DIR)), READ_ONLY_EXEC_MODE);
        assert_eq!(mode(&root.join("upstream/SKILL.md")), READ_ONLY_MODE);
        assert_eq!(mode(&root.join("upstream/run.sh")), READ_ONLY_EXEC_MODE);
        assert_eq!(mode(&root.join(BASE_DIR)), READ_ONLY_EXEC_MODE);
        assert_eq!(mode(&root.join(DIFFS_DIR)), READ_ONLY_EXEC_MODE);
        assert_eq!(mode(&root.join("diffs/upstream.diff")), READ_ONLY_MODE);
        assert!(fs::write(root.join("upstream/new.md"), "x").is_err());
        assert_eq!(mode(&root.join("output/SKILL.md")) & 0o200, 0o200);
        fs::write(root.join("output/new.md"), "x").unwrap();

        // Read-only directories, including ones the agent locked, are
        // removed with the staging directory.
        fs::create_dir(root.join("output/locked")).unwrap();
        fs::write(root.join("output/locked/file"), "x").unwrap();
        fs::set_permissions(
            root.join("output/locked"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
        drop(staging);
        assert!(!root.exists());
    }

    #[test]
    fn drop_does_not_follow_symbolic_links_out_of_the_staging_directory() {
        let outside = tempfile::TempDir::new().unwrap();
        let locked = outside.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let staging = Staging::create(job(
            Inputs::Composition(BTreeMap::from([(
                Name::new("a").unwrap(),
                InputRevisions {
                    old: None,
                    new: tree(&[("SKILL.md", "x")]),
                },
            )])),
            None,
        ))
        .unwrap();
        std::os::unix::fs::symlink(&locked, staging.output_dir().join("link")).unwrap();
        drop(staging);
        assert_eq!(mode(&locked), 0o500);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
