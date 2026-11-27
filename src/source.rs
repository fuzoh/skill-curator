//! Upstream Sources: a local cache of each Source's Git history, fetched with
//! the system `git` binary, and the strict extraction of a Skill directory at
//! a Revision with its license files.
//!
//! ```text
//! <cache>/sources/<source>-<url hash>.git   bare repository per Source URL
//! ```
//!
//! The cache keeps every commit it has fetched, so a pinned Revision stays
//! available after an upstream history rewrite once it has been fetched.
//! Extraction never checks out a working tree and never runs anything from
//! the Source: it lists the tree at the Revision, refuses symbolic links,
//! submodules, and unsafe paths, and writes the Skill's regular files into a
//! fresh read-only temporary directory.

mod git;
mod license;
mod tree;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use crate::collection::lock::{LicenseFile, Provenance, Revision};
use crate::collection::manifest::{Source, branch_problem, git_argument_problem};
use crate::safety::{HomeDir, Name, RelativePath};
pub use git::Git;
pub use license::{LicenseWarning, LicenseWarningKind, identify as identify_license};
pub use tree::{EntryProblem, NotFoundReason, RejectedEntry};
use tree::{Refusal, Selection};

/// Directory of the tool's cache below the user's cache directory.
const CACHE_DIR_NAME: &str = "skill-curator";
/// Directory of the per-Source repositories inside the cache.
const SOURCES_DIR: &str = "sources";
/// Hexadecimal digits of the URL hash in a cache repository name.
const URL_HASH_LEN: usize = 16;

/// Failure to fetch a Source or extract a Skill from it.
#[derive(Debug)]
#[non_exhaustive]
pub enum SourceError {
    /// The `git` program was not found.
    GitMissing {
        /// Program that was looked up.
        program: OsString,
    },
    /// The `git` program exists but could not be started.
    GitSpawn {
        /// Program that was started.
        program: OsString,
        /// Underlying error.
        source: io::Error,
    },
    /// A `git` command failed.
    Git {
        /// The command, such as `git fetch`.
        command: String,
        /// What `git` reported.
        stderr: String,
    },
    /// `git` succeeded but printed something unexpected.
    UnexpectedOutput {
        /// The command, such as `git rev-parse`.
        command: String,
        /// What was wrong with the output.
        detail: String,
    },
    /// A cache or extraction directory could not be read or written.
    Io {
        /// What was being done, such as `create`.
        action: &'static str,
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The Source's URL or branch could be misread by `git`.
    InvalidSource {
        /// Source name in the Manifest.
        source_name: Name,
        /// What is wrong.
        problem: String,
    },
    /// The Revision is neither in the cache nor available upstream, for
    /// example after an upstream history rewrite. Callers comparing an old
    /// and a new Revision can fall back to the new one alone.
    RevisionUnreachable {
        /// Source name in the Manifest.
        source_name: Name,
        /// Repository URL.
        url: String,
        /// The missing Revision.
        revision: Revision,
        /// What `git` reported when asked for it.
        detail: String,
    },
    /// `upstream_path` is not a Skill directory at the Revision. Nothing is
    /// chosen in its place.
    SkillNotFound {
        /// Source name in the Manifest.
        source_name: Name,
        /// Revision looked at.
        revision: Revision,
        /// Path that was looked up.
        upstream_path: RelativePath,
        /// Why it is not a Skill.
        reason: NotFoundReason,
        /// Skill directories at the Revision, nearest first.
        candidates: Vec<RelativePath>,
    },
    /// The Skill directory holds entries that cannot be extracted safely.
    UnsafeSkill {
        /// Source name in the Manifest.
        source_name: Name,
        /// Revision looked at.
        revision: Revision,
        /// Skill directory.
        upstream_path: RelativePath,
        /// Every refused entry.
        entries: Vec<RejectedEntry>,
    },
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitMissing { program } => write!(
                f,
                "system `git` was not found (looked for `{}`); install Git and make sure `git` is on PATH, since skill-curator fetches Sources with it",
                program.to_string_lossy()
            ),
            Self::GitSpawn { program, source } => {
                write!(f, "cannot run `{}`: {source}", program.to_string_lossy())
            }
            Self::Git { command, stderr } => write!(f, "`{command}` failed: {stderr}"),
            Self::UnexpectedOutput { command, detail } => {
                write!(f, "unexpected output from `{command}`: {detail}")
            }
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "cannot {action} {}: {source}", path.display()),
            Self::InvalidSource {
                source_name,
                problem,
            } => write!(f, "Source `{source_name}`: {problem}"),
            Self::RevisionUnreachable {
                source_name,
                url,
                revision,
                detail,
            } => {
                write!(
                    f,
                    "Revision {revision} of Source `{source_name}` is not in the local cache and {url} no longer provides it; its upstream history was probably rewritten"
                )?;
                if !detail.is_empty() {
                    write!(f, " (git: {detail})")?;
                }
                Ok(())
            }
            Self::SkillNotFound {
                source_name,
                revision,
                upstream_path,
                reason,
                candidates,
            } => {
                write!(
                    f,
                    "Source `{source_name}` has no Skill at `{upstream_path}` at Revision {revision}: {reason}; set `upstream_path` in manifest.toml to the Skill's current directory"
                )?;
                if candidates.is_empty() {
                    write!(
                        f,
                        ". No directory with a `SKILL.md` exists at that Revision."
                    )
                } else {
                    write!(f, ". Skill directories found at that Revision:")?;
                    for candidate in candidates {
                        write!(f, "\n  - {candidate}")?;
                    }
                    Ok(())
                }
            }
            Self::UnsafeSkill {
                source_name,
                revision,
                upstream_path,
                entries,
            } => {
                write!(
                    f,
                    "Skill `{upstream_path}` of Source `{source_name}` at Revision {revision} cannot be extracted safely:"
                )?;
                for entry in entries {
                    write!(f, "\n  - {entry}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GitSpawn { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_error(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> SourceError {
    move |source| SourceError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

/// Local cache of Source repositories.
#[derive(Debug, Clone)]
pub struct SourceCache {
    root: PathBuf,
    git: Git,
}

impl SourceCache {
    /// A cache stored under `root`, using `git` for every operation. Nothing
    /// is created until a Source is fetched.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, git: Git) -> Self {
        Self {
            root: root.into(),
            git,
        }
    }

    /// The default cache directory: `$XDG_CACHE_HOME/skill-curator` when
    /// `xdg_cache_home` is an absolute path, `~/.cache/skill-curator`
    /// otherwise, as the XDG Base Directory specification requires.
    #[must_use]
    pub fn default_root(xdg_cache_home: Option<&OsStr>, home: &HomeDir) -> PathBuf {
        let base = xdg_cache_home
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .map_or_else(|| home.as_path().join(".cache"), Path::to_path_buf);
        base.join(CACHE_DIR_NAME)
    }

    /// The cache directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Bare repository caching the Source at `url`: named after the Source
    /// for readability, keyed by a hash of the URL so a changed URL starts a
    /// new cache.
    fn repository(&self, name: &Name, url: &str) -> PathBuf {
        let digest = Sha256::digest(url.as_bytes());
        let hash = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root
            .join(SOURCES_DIR)
            .join(format!("{name}-{}.git", &hash[..URL_HASH_LEN]))
    }

    /// Updates the cache of Source `name` and returns its candidate
    /// Revision: the tip of `source.branch`, or of the repository's default
    /// branch when no branch is set. The first call clones the Source.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when `git` is missing or fails (unknown
    /// repository, missing branch, network failure), when the Source's URL
    /// or branch could be misread by `git`, or when the cache cannot be
    /// written.
    pub fn fetch(&self, name: &Name, source: &Source) -> Result<Revision, SourceError> {
        check_source(name, source)?;
        let repository = self.ensure_repository(name, source)?;
        let (wanted, local) = match &source.branch {
            Some(branch) => (
                format!("refs/heads/{branch}"),
                format!("refs/skill-curator/branches/{branch}"),
            ),
            None => ("HEAD".to_owned(), "refs/skill-curator/default".to_owned()),
        };
        self.fetch_from(&repository, &source.url, &format!("+{wanted}:{local}"))?;
        let output = self.git.run(
            Some(&repository),
            &[
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--end-of-options"),
                OsStr::new(&format!("{local}^{{commit}}")),
            ],
        )?;
        let text = String::from_utf8_lossy(&output);
        Revision::new(text.trim()).map_err(|err| SourceError::UnexpectedOutput {
            command: "git rev-parse".to_owned(),
            detail: err.to_string(),
        })
    }

    /// Extracts the Skill directory `upstream_path` of Source `name` at
    /// `revision` into a new read-only temporary directory, and detects its
    /// license files.
    ///
    /// The Revision is read from the cache; when missing, it is fetched by
    /// identifier. Only regular files are extracted: symbolic links,
    /// submodules, and paths with `..`, `.`, or `.git` components are
    /// refused, all of them listed in the error.
    ///
    /// # Errors
    ///
    /// Returns [`SourceError::RevisionUnreachable`] when the Revision is
    /// neither cached nor available upstream, [`SourceError::SkillNotFound`]
    /// with nearby Skill directories when `upstream_path` is not a Skill
    /// directory at the Revision, [`SourceError::UnsafeSkill`] when it holds
    /// entries that cannot be extracted safely, and another [`SourceError`]
    /// when `git` or the filesystem fails.
    pub fn extract(
        &self,
        name: &Name,
        source: &Source,
        revision: &Revision,
        upstream_path: &RelativePath,
    ) -> Result<ExtractedSkill, SourceError> {
        check_source(name, source)?;
        let repository = self.ensure_repository(name, source)?;
        self.ensure_revision(&repository, name, source, revision)?;

        let listing = self.git.run(
            Some(&repository),
            &[
                OsStr::new("ls-tree"),
                OsStr::new("-r"),
                OsStr::new("-z"),
                OsStr::new(revision.as_str()),
            ],
        )?;
        let entries =
            tree::parse_listing(&listing).map_err(|detail| SourceError::UnexpectedOutput {
                command: "git ls-tree".to_owned(),
                detail,
            })?;

        let files = match tree::select_skill(&entries, upstream_path) {
            Selection::Files(files) => files,
            Selection::NotFound(reason) => {
                return Err(SourceError::SkillNotFound {
                    source_name: name.clone(),
                    revision: revision.clone(),
                    upstream_path: upstream_path.clone(),
                    reason,
                    candidates: tree::nearby_skills(&entries, upstream_path),
                });
            }
            Selection::Unsafe(refused) => {
                let mut rejected = Vec::with_capacity(refused.len());
                for entry in refused {
                    let problem = match entry.kind {
                        Refusal::Symlink { oid } => {
                            let target =
                                String::from_utf8_lossy(&self.read_blob(&repository, oid)?)
                                    .into_owned();
                            EntryProblem::Symlink {
                                escapes: tree::link_escapes(&entry.path, &target),
                                target,
                            }
                        }
                        Refusal::Other(problem) => problem,
                    };
                    rejected.push(RejectedEntry {
                        path: entry.path,
                        problem,
                    });
                }
                return Err(SourceError::UnsafeSkill {
                    source_name: name.clone(),
                    revision: revision.clone(),
                    upstream_path: upstream_path.clone(),
                    entries: rejected,
                });
            }
        };

        let dir = ReadOnlyDir::create()?;
        for file in &files {
            let contents = self.read_blob(&repository, file.oid)?;
            dir.write(&file.path, &contents)?;
        }
        dir.seal(
            &files
                .iter()
                .map(|file| (&file.path, file.executable))
                .collect::<Vec<_>>(),
        )?;

        let mut licenses = Vec::new();
        for (path, oid) in tree::license_files(&entries, upstream_path) {
            let text = self.read_blob(&repository, oid)?;
            licenses.push(LicenseFile {
                path,
                summary: license::identify(&text),
            });
        }

        Ok(ExtractedSkill {
            dir,
            source_name: name.clone(),
            url: source.url.clone(),
            upstream_path: upstream_path.clone(),
            revision: revision.clone(),
            licenses,
        })
    }

    /// The cache repository of the Source, cloned first when absent. The
    /// clone goes to a temporary directory and is moved into place once
    /// complete, so an interrupted clone leaves no half-made cache.
    fn ensure_repository(&self, name: &Name, source: &Source) -> Result<PathBuf, SourceError> {
        let repository = self.repository(name, &source.url);
        if repository.is_dir() {
            return Ok(repository);
        }
        let sources = self.root.join(SOURCES_DIR);
        fs::create_dir_all(&sources).map_err(io_error("create", &sources))?;
        let staging = tempfile::Builder::new()
            .prefix(".clone-")
            .tempdir_in(&sources)
            .map_err(io_error("create a directory in", &sources))?;
        let clone = staging.path().join("repository.git");
        self.git.run(
            None,
            &[
                OsStr::new("clone"),
                OsStr::new("--bare"),
                OsStr::new("--quiet"),
                OsStr::new("--no-tags"),
                // No template: neither hooks nor configuration from the
                // user's template directory reach the cache.
                OsStr::new("--template="),
                OsStr::new("--end-of-options"),
                OsStr::new(&source.url),
                clone.as_os_str(),
            ],
        )?;
        if let Err(err) = fs::rename(&clone, &repository) {
            // Another run may have cloned the same Source meanwhile.
            if !repository.is_dir() {
                return Err(io_error("move the new clone to", &repository)(err));
            }
        }
        Ok(repository)
    }

    /// Fetches `refspec` from `url` into the cache, without tags or
    /// submodules.
    fn fetch_from(&self, repository: &Path, url: &str, refspec: &str) -> Result<(), SourceError> {
        self.git
            .run(
                Some(repository),
                &[
                    OsStr::new("fetch"),
                    OsStr::new("--quiet"),
                    OsStr::new("--no-tags"),
                    OsStr::new("--no-recurse-submodules"),
                    OsStr::new("--no-write-fetch-head"),
                    OsStr::new("--end-of-options"),
                    OsStr::new(url),
                    OsStr::new(refspec),
                ],
            )
            .map(drop)
    }

    /// Makes sure `revision` is a commit of the cache, fetching it by
    /// identifier when missing, and keeps it there with a reference.
    fn ensure_revision(
        &self,
        repository: &Path,
        name: &Name,
        source: &Source,
        revision: &Revision,
    ) -> Result<(), SourceError> {
        if !self.has_commit(repository, revision)? {
            let unreachable = |detail: String| SourceError::RevisionUnreachable {
                source_name: name.clone(),
                url: source.url.clone(),
                revision: revision.clone(),
                detail,
            };
            match self.fetch_from(repository, &source.url, revision.as_str()) {
                Ok(()) => {}
                Err(SourceError::Git { stderr, .. }) => return Err(unreachable(stderr)),
                Err(err) => return Err(err),
            }
            if !self.has_commit(repository, revision)? {
                return Err(unreachable(String::new()));
            }
        }
        // Pin the Revision so it outlives upstream rewrites of the branch it
        // came from.
        let pin = format!("refs/skill-curator/revisions/{revision}");
        self.git
            .run(
                Some(repository),
                &[
                    OsStr::new("update-ref"),
                    OsStr::new(&pin),
                    OsStr::new(revision.as_str()),
                ],
            )
            .map(drop)
    }

    /// Whether `revision` names a commit present in the cache.
    fn has_commit(&self, repository: &Path, revision: &Revision) -> Result<bool, SourceError> {
        let object = format!("{revision}^{{commit}}");
        match self.git.run(
            Some(repository),
            &[
                OsStr::new("cat-file"),
                OsStr::new("-e"),
                OsStr::new(&object),
            ],
        ) {
            Ok(_) => Ok(true),
            Err(SourceError::Git { .. }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn read_blob(&self, repository: &Path, oid: &str) -> Result<Vec<u8>, SourceError> {
        self.git.run(
            Some(repository),
            &[OsStr::new("cat-file"), OsStr::new("blob"), OsStr::new(oid)],
        )
    }
}

/// Rejects a Source whose URL or branch `git` could misread as an option,
/// refspec, or revision expression. The Manifest loader reports the same
/// problems; this guards Sources built in code.
fn check_source(name: &Name, source: &Source) -> Result<(), SourceError> {
    let problem = git_argument_problem("repository URL", &source.url)
        .or_else(|| source.branch.as_deref().and_then(branch_problem));
    match problem {
        Some(problem) => Err(SourceError::InvalidSource {
            source_name: name.clone(),
            problem,
        }),
        None => Ok(()),
    }
}

/// A Skill directory extracted from a Source at a Revision, with the license
/// files that apply to it. The directory is read-only and removed on drop.
#[derive(Debug)]
pub struct ExtractedSkill {
    dir: ReadOnlyDir,
    source_name: Name,
    url: String,
    upstream_path: RelativePath,
    revision: Revision,
    licenses: Vec<LicenseFile>,
}

impl ExtractedSkill {
    /// Directory holding the Skill's files, `SKILL.md` at its top.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Revision the Skill was extracted at.
    #[must_use]
    pub fn revision(&self) -> &Revision {
        &self.revision
    }

    /// License files found at the Source root, in each directory leading to
    /// the Skill, and in the Skill directory, from the root down.
    #[must_use]
    pub fn licenses(&self) -> &[LicenseFile] {
        &self.licenses
    }

    /// The warning to show when no license was found or one could not be
    /// identified.
    #[must_use]
    pub fn license_warning(&self) -> Option<LicenseWarning> {
        LicenseWarning::check(&self.source_name, &self.upstream_path, &self.licenses)
    }

    /// Provenance entry for `lock.toml`; `input` names the Composition input
    /// and is `None` for Copy and Adapt Skills.
    #[must_use]
    pub fn provenance(&self, input: Option<Name>) -> Provenance {
        Provenance {
            input,
            source: self.source_name.clone(),
            url: self.url.clone(),
            upstream_path: self.upstream_path.clone(),
            revision: self.revision.clone(),
            licenses: self.licenses.clone(),
        }
    }
}

/// A temporary directory whose files and directories are made read-only
/// once written, and made writable again just before removal.
#[derive(Debug)]
struct ReadOnlyDir {
    dir: TempDir,
}

impl ReadOnlyDir {
    fn create() -> Result<Self, SourceError> {
        let dir = tempfile::Builder::new()
            .prefix("skill-curator-")
            .tempdir()
            .map_err(io_error(
                "create a temporary directory in",
                &std::env::temp_dir(),
            ))?;
        Ok(Self { dir })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes a new file, creating its parent directories. Never follows or
    /// replaces an existing entry.
    fn write(&self, relative: &RelativePath, contents: &[u8]) -> Result<(), SourceError> {
        let path = relative.join_to(self.path());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error("create", parent))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error("create", &path))?;
        file.write_all(contents).map_err(io_error("write", &path))
    }

    /// Makes `files` read-only (keeping the executable bit), then every
    /// directory, deepest first.
    fn seal(&self, files: &[(&RelativePath, bool)]) -> Result<(), SourceError> {
        let mut dirs = vec![self.path().to_path_buf()];
        for (relative, executable) in files {
            let path = relative.join_to(self.path());
            let mode = if *executable { 0o555 } else { 0o444 };
            set_mode(&path, mode)?;
            dirs.extend(
                path.ancestors()
                    .skip(1)
                    .take_while(|dir| dir.starts_with(self.path()) && *dir != self.path())
                    .map(Path::to_path_buf),
            );
        }
        dirs.sort();
        dirs.dedup();
        // Children sort after their parents.
        for dir in dirs.iter().rev() {
            set_mode(dir, 0o555)?;
        }
        Ok(())
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), SourceError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(io_error("set permissions on", path))
}

impl Drop for ReadOnlyDir {
    fn drop(&mut self) {
        // Removing entries needs write permission on their directory. Only
        // real directories are touched; links are never followed.
        fn make_writable(dir: &Path) {
            if fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).is_err() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_root_follows_xdg() {
        let home = HomeDir::new("/home/user").unwrap();
        let cases = [
            (None, "/home/user/.cache/skill-curator"),
            (Some(""), "/home/user/.cache/skill-curator"),
            (Some("relative/cache"), "/home/user/.cache/skill-curator"),
            (Some("/var/cache/me"), "/var/cache/me/skill-curator"),
        ];
        for (xdg, expected) in cases {
            assert_eq!(
                SourceCache::default_root(xdg.map(OsStr::new), &home),
                Path::new(expected),
                "{xdg:?}"
            );
        }
    }

    #[test]
    fn cache_repositories_are_keyed_by_url() {
        let cache = SourceCache::new("/cache", Git::system());
        let name = Name::new("mattpocock").unwrap();
        let first = cache.repository(&name, "https://example.com/a.git");
        assert!(first.starts_with("/cache/sources"));
        let file_name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(file_name.starts_with("mattpocock-") && file_name.ends_with(".git"));
        assert_eq!(
            file_name.len(),
            "mattpocock-".len() + URL_HASH_LEN + ".git".len()
        );
        assert_eq!(first, cache.repository(&name, "https://example.com/a.git"));
        assert_ne!(first, cache.repository(&name, "https://example.com/b.git"));
    }

    #[test]
    fn sources_git_could_misread_are_refused() {
        let name = Name::new("s").unwrap();
        let cases = [
            Source {
                url: "--upload-pack=touch /tmp/x".to_owned(),
                branch: None,
            },
            Source {
                url: "https://example.com/a.git".to_owned(),
                branch: Some("main:refs/heads/x".to_owned()),
            },
        ];
        for source in cases {
            let err = check_source(&name, &source).unwrap_err();
            assert!(matches!(err, SourceError::InvalidSource { .. }), "{err:?}");
        }
        let fine = Source {
            url: "https://example.com/a.git".to_owned(),
            branch: Some("main".to_owned()),
        };
        assert!(check_source(&name, &fine).is_ok());
    }

    #[test]
    fn read_only_directories_are_removed_on_drop() {
        let dir = ReadOnlyDir::create().unwrap();
        let file = RelativePath::new("a/b/SKILL.md").unwrap();
        let script = RelativePath::new("a/run.sh").unwrap();
        dir.write(&file, b"x").unwrap();
        dir.write(&script, b"y").unwrap();
        assert!(
            dir.write(&file, b"again").is_err(),
            "existing files are never replaced"
        );
        dir.seal(&[(&file, false), (&script, true)]).unwrap();

        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file.join_to(dir.path())), 0o444);
        assert_eq!(mode(&script.join_to(dir.path())), 0o555);
        assert_eq!(mode(&dir.path().join("a/b")), 0o555);
        assert_eq!(mode(dir.path()), 0o555);

        let root = dir.path().to_path_buf();
        drop(dir);
        assert!(!root.exists());
    }
}
