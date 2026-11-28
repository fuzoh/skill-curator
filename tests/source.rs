//! Source cache, Revision fetching, strict Skill extraction, and license
//! detection, against local fixture Git repositories. No test touches the
//! network or the user's Git configuration.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use skill_curator::collection::SKILL_FILE;
use skill_curator::collection::attribution;
use skill_curator::collection::lock::{Lock, LockedSkill, Revision};
use skill_curator::collection::manifest::Source;
use skill_curator::hash::tree_digest;
use skill_curator::safety::{Name, PathError, RelativePath};
use skill_curator::source::{
    EntryProblem, ExtractedSkill, Git, LicenseWarningKind, NotFoundReason, SourceCache, SourceError,
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

const MIT: &str = "MIT License\n\nCopyright (c) 2026 Upstream\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software.\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n";
const APACHE: &str = "                                 Apache License\n                           Version 2.0, January 2004\n";

/// A test sandbox: a home directory with no Git configuration, upstream
/// repositories, and a Source cache.
struct Sandbox {
    dir: TempDir,
    /// Global Git configuration file, `/dev/null` unless set by the test.
    git_config: PathBuf,
}

impl Sandbox {
    fn new() -> Result<Self, Box<dyn Error>> {
        let dir = TempDir::new()?;
        fs::create_dir(dir.path().join("home"))?;
        Ok(Self {
            dir,
            git_config: PathBuf::from("/dev/null"),
        })
    }

    /// A sandbox whose global Git configuration, read by the cache and the
    /// upstream alike, is `contents`, as a user's own settings would be.
    fn with_git_config(contents: &str) -> Result<Self, Box<dyn Error>> {
        let mut sandbox = Self::new()?;
        let config = sandbox.dir.path().join("gitconfig");
        fs::write(&config, contents)?;
        sandbox.git_config = config;
        Ok(sandbox)
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    /// Environment that isolates `git` from the user's configuration.
    fn env(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.home()),
            ("GIT_CONFIG_GLOBAL", self.git_config.clone()),
            ("GIT_CONFIG_NOSYSTEM", PathBuf::from("1")),
        ]
    }

    fn git(&self) -> Git {
        self.env()
            .into_iter()
            .fold(Git::system(), |git, (key, value)| git.env(key, value))
    }

    fn cache(&self) -> SourceCache {
        self.cache_at("cache")
    }

    fn cache_at(&self, dir: &str) -> SourceCache {
        SourceCache::new(self.dir.path().join(dir), self.git())
    }

    /// A new bare upstream repository on branch `main`, with a separate
    /// working tree to prepare its commits in.
    fn upstream(&self, name: &str) -> Result<Upstream, Box<dyn Error>> {
        let upstream = Upstream {
            dir: self.dir.path().join(name),
            repository: self.dir.path().join(format!("{name}.git")),
            env: self.env(),
        };
        fs::create_dir(&upstream.dir)?;
        let mut init = upstream.command();
        init.args(["init", "--quiet", "--bare", "--initial-branch=main"])
            .arg(&upstream.repository);
        finish(init.output()?, &["init"])?;
        Ok(upstream)
    }
}

/// An upstream Source: a bare repository, which the Source URL points at,
/// and a working tree outside it where the tests prepare commits.
struct Upstream {
    /// Working tree.
    dir: PathBuf,
    /// Bare repository.
    repository: PathBuf,
    env: Vec<(&'static str, PathBuf)>,
}

impl Upstream {
    fn source(&self) -> Source {
        Source {
            url: format!("file://{}", self.repository.display()),
            branch: None,
        }
    }

    /// `git` in the working tree, isolated from the user's configuration.
    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(&self.dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .envs(self.env.iter().map(|(key, value)| (*key, value)));
        command
    }

    fn git(&self, args: &[&str]) -> Result<String, Box<dyn Error>> {
        self.git_with_input(args, None)
    }

    /// Runs `git` on the bare repository and the working tree.
    fn git_with_input(&self, args: &[&str], input: Option<&str>) -> Result<String, Box<dyn Error>> {
        let mut git_dir = OsString::from("--git-dir=");
        git_dir.push(&self.repository);
        let mut work_tree = OsString::from("--work-tree=");
        work_tree.push(&self.dir);
        let mut child = self
            .command()
            .arg(git_dir)
            .arg(work_tree)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Upstream")
            .env("GIT_AUTHOR_EMAIL", "upstream@example.com")
            .env("GIT_COMMITTER_NAME", "Upstream")
            .env("GIT_COMMITTER_EMAIL", "upstream@example.com")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or("no stdin")?
                .write_all(input.as_bytes())?;
        }
        drop(child.stdin.take());
        finish(child.wait_with_output()?, args)
    }

    fn write(&self, relative: &str, contents: &str) -> TestResult {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
        Ok(())
    }

    /// Commits every change and returns the new Revision.
    fn commit(&self, message: &str) -> Result<Revision, Box<dyn Error>> {
        self.git(&["add", "--all"])?;
        self.git(&["commit", "--quiet", "--message", message])?;
        self.head()
    }

    fn head(&self) -> Result<Revision, Box<dyn Error>> {
        Ok(Revision::new(&self.git(&["rev-parse", "HEAD"])?)?)
    }

    /// Rewrites `main` to `revision` and deletes every unreachable object, as
    /// a force-push followed by server-side garbage collection would.
    fn rewrite_to(&self, revision: &Revision) -> TestResult {
        self.git(&["reset", "--quiet", "--hard", revision.as_str()])?;
        self.git(&["update-ref", "-d", "ORIG_HEAD"])?;
        self.git(&["reflog", "expire", "--expire=now", "--all"])?;
        self.git(&["gc", "--quiet", "--prune=now"])?;
        Ok(())
    }

    fn has_commit(&self, revision: &Revision) -> bool {
        self.git(&["cat-file", "-e", &format!("{revision}^{{commit}}")])
            .is_ok()
    }
}

/// The trimmed standard output of a successful `git` command.
fn finish(output: Output, args: &[&str]) -> Result<String, Box<dyn Error>> {
    if !output.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn name(raw: &str) -> Result<Name, Box<dyn Error>> {
    Ok(Name::new(raw)?)
}

fn path(raw: &str) -> Result<RelativePath, Box<dyn Error>> {
    Ok(RelativePath::new(raw)?)
}

fn read(skill: &ExtractedSkill, relative: &str) -> Result<String, Box<dyn Error>> {
    Ok(fs::read_to_string(skill.path().join(relative))?)
}

fn skill_md(version: &str) -> String {
    format!("---\nname: grill-me\ndescription: Grill the user.\n---\nVersion {version}\n")
}

/// The path and problem of every entry an `UnsafeSkill` error refused.
fn refused_entries(err: &SourceError) -> Result<Vec<(&str, EntryProblem)>, Box<dyn Error>> {
    let SourceError::UnsafeSkill { entries, .. } = err else {
        return Err(format!("expected UnsafeSkill, got {err:?}").into());
    };
    Ok(entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.problem.clone()))
        .collect())
}

/// Every file below `dir`, relative and sorted.
fn files(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                let relative = entry
                    .path()
                    .strip_prefix(dir)?
                    .to_string_lossy()
                    .into_owned();
                found.push(relative);
            }
        }
    }
    found.sort();
    Ok(found)
}

#[test]
fn clones_fetches_and_extracts_old_and_new_revisions() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("LICENSE", MIT)?;
    upstream.write("skills/grill-me/SKILL.md", &skill_md("1"))?;
    upstream.write("skills/grill-me/references/questions.md", "Why?\n")?;
    upstream.write("skills/other/SKILL.md", "other\n")?;
    let r_old = upstream.commit("first")?;

    assert_eq!(
        upstream.git(&["config", "core.bare"])?,
        "true",
        "the Source is a bare repository"
    );

    let cache = sandbox.cache();
    let source = upstream.source();
    let mattpocock = name("mattpocock")?;
    assert_eq!(cache.fetch(&mattpocock, &source)?, r_old);

    upstream.write("skills/grill-me/SKILL.md", &skill_md("2"))?;
    upstream.write("skills/grill-me/scripts/run.sh", "echo never run\n")?;
    fs::set_permissions(
        upstream.dir.join("skills/grill-me/scripts/run.sh"),
        fs::Permissions::from_mode(0o755),
    )?;
    let r_new = upstream.commit("second")?;
    assert_eq!(cache.fetch(&mattpocock, &source)?, r_new);
    assert_eq!(
        fs::read_dir(cache.root().join("sources"))?.count(),
        1,
        "one cache repository per Source"
    );

    let skill = path("skills/grill-me")?;
    let old = cache.extract(&mattpocock, &source, &r_old, &skill)?;
    let new = cache.extract(&mattpocock, &source, &r_new, &skill)?;
    assert_eq!(old.revision(), &r_old);
    assert_eq!(new.revision(), &r_new);
    assert_eq!(read(&old, SKILL_FILE)?, skill_md("1"));
    assert_eq!(read(&new, SKILL_FILE)?, skill_md("2"));
    assert_eq!(files(old.path())?, ["SKILL.md", "references/questions.md"]);
    assert_eq!(
        files(new.path())?,
        ["SKILL.md", "references/questions.md", "scripts/run.sh"]
    );

    // The extraction is read-only, keeps the executable bit, and is removed
    // on drop.
    let mode = |relative: &str| -> Result<u32, Box<dyn Error>> {
        Ok(fs::metadata(new.path().join(relative))?
            .permissions()
            .mode()
            & 0o777)
    };
    assert_eq!(mode(SKILL_FILE)?, 0o444);
    assert_eq!(mode("scripts/run.sh")?, 0o555);
    assert_eq!(mode("scripts")?, 0o555);
    assert!(fs::write(new.path().join(SKILL_FILE), "edited").is_err());
    let extracted = new.path().to_path_buf();
    drop(new);
    assert!(!extracted.exists());
    Ok(())
}

#[test]
fn fetches_the_configured_branch() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "main\n")?;
    let main = upstream.commit("main")?;
    upstream.git(&["switch", "--quiet", "--create", "next"])?;
    upstream.write("skills/a/SKILL.md", "next\n")?;
    let next = upstream.commit("next")?;
    upstream.git(&["switch", "--quiet", "main"])?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let mut source = upstream.source();
    assert_eq!(cache.fetch(&source_name, &source)?, main);
    source.branch = Some("next".to_owned());
    assert_eq!(cache.fetch(&source_name, &source)?, next);

    source.branch = Some("gone".to_owned());
    let err = cache
        .fetch(&source_name, &source)
        .err()
        .ok_or("a missing branch must fail")?;
    assert!(
        matches!(&err, SourceError::Git { command, .. } if command == "git fetch"),
        "{err:?}"
    );
    Ok(())
}

#[test]
fn a_branch_can_change_to_a_nested_or_parent_name() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "main\n")?;
    upstream.commit("main")?;
    let mut tips = Vec::new();
    for branch in ["release", "release-next"] {
        upstream.git(&["switch", "--quiet", "--create", branch, "main"])?;
        upstream.write("skills/a/SKILL.md", &format!("{branch}\n"))?;
        tips.push(upstream.commit(branch)?);
    }
    // Git cannot hold `release` and `release/v2` at once, so the second
    // branch is published under its nested name in a separate step.
    let nested = tips.pop().ok_or("no nested tip")?;
    let release = tips.pop().ok_or("no release tip")?;
    upstream.git(&["switch", "--quiet", "main"])?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let mut source = upstream.source();
    source.branch = Some("release".to_owned());
    assert_eq!(cache.fetch(&source_name, &source)?, release);

    upstream.git(&["branch", "--quiet", "--delete", "--force", "release"])?;
    upstream.git(&["branch", "--quiet", "--move", "release-next", "release/v2"])?;
    source.branch = Some("release/v2".to_owned());
    assert_eq!(cache.fetch(&source_name, &source)?, nested);

    upstream.git(&["branch", "--quiet", "--move", "release/v2", "release"])?;
    source.branch = Some("release".to_owned());
    assert_eq!(cache.fetch(&source_name, &source)?, nested);
    Ok(())
}

#[test]
fn an_unreachable_source_is_a_fetch_failure_not_a_rewrite() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "1\n")?;
    let first = upstream.commit("first")?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    assert_eq!(cache.fetch(&source_name, &source)?, first);
    upstream.write("skills/a/SKILL.md", "2\n")?;
    let uncached = upstream.commit("second")?;

    // The Source goes offline, as on a network or authentication failure.
    let offline = sandbox.dir.path().join("offline.git");
    fs::rename(&upstream.repository, &offline)?;
    let err = cache
        .extract(&source_name, &source, &uncached, &path("skills/a")?)
        .err()
        .ok_or("an unreachable Source must fail")?;
    assert!(
        matches!(&err, SourceError::Git { command, .. } if command == "git fetch"),
        "{err:?}"
    );
    assert!(!err.to_string().contains("rewritten"), "{err}");
    // Cached Revisions still extract offline.
    let cached = cache.extract(&source_name, &source, &first, &path("skills/a")?)?;
    assert_eq!(read(&cached, SKILL_FILE)?, "1\n");

    // Back online, the same Revision is fetched by identifier.
    fs::rename(&offline, &upstream.repository)?;
    let fetched = cache.extract(&source_name, &source, &uncached, &path("skills/a")?)?;
    assert_eq!(read(&fetched, SKILL_FILE)?, "2\n");
    Ok(())
}

#[test]
fn an_unreachable_old_revision_is_reported_distinctly() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "1\n")?;
    let first = upstream.commit("first")?;
    upstream.write("skills/a/SKILL.md", "2\n")?;
    let rewritten = upstream.commit("second")?;
    upstream.rewrite_to(&first)?;
    assert!(
        !upstream.has_commit(&rewritten),
        "fixture must drop the commit"
    );

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    assert_eq!(cache.fetch(&source_name, &source)?, first);

    let never_existed = Revision::new(&"0".repeat(40))?;
    for missing in [&rewritten, &never_existed] {
        let err = cache
            .extract(&source_name, &source, missing, &path("skills/a")?)
            .err()
            .ok_or("an unreachable Revision must fail")?;
        let SourceError::RevisionUnreachable { revision, .. } = &err else {
            return Err(format!("expected RevisionUnreachable, got {err:?}").into());
        };
        assert_eq!(revision, missing);
        let message = err.to_string();
        assert!(message.contains(missing.as_str()), "{message}");
        assert!(
            message.contains("history was probably rewritten"),
            "{message}"
        );
    }

    // The new Revision still extracts, so callers can fall back to it.
    let current = cache.extract(&source_name, &source, &first, &path("skills/a")?)?;
    assert_eq!(read(&current, SKILL_FILE)?, "1\n");
    Ok(())
}

/// Over protocol v0 or v1, which servers without protocol v2 support and
/// users with `protocol.version=0` get, Git refuses to fetch any commit that
/// is not a branch tip by identifier. Revisions the branches still reach
/// must extract anyway, and rewritten ones must stay `RevisionUnreachable`.
#[test]
fn protocol_v0_fetches_revisions_through_branches() -> TestResult {
    let sandbox = Sandbox::with_git_config("[protocol]\n\tversion = 0\n")?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "1\n")?;
    let first = upstream.commit("first")?;
    upstream.write("skills/a/SKILL.md", "rewritten\n")?;
    let rewritten = upstream.commit("rewritten")?;
    upstream.rewrite_to(&first)?;
    assert!(!upstream.has_commit(&rewritten));
    upstream.git(&["branch", "--quiet", "release"])?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    assert_eq!(cache.fetch(&source_name, &source)?, first);

    // Another machine moved the lock to a commit that is no longer the tip.
    upstream.write("skills/a/SKILL.md", "behind\n")?;
    let behind = upstream.commit("behind")?;
    upstream.write("skills/a/SKILL.md", "tip\n")?;
    upstream.commit("tip")?;
    let skill = cache.extract(&source_name, &source, &behind, &path("skills/a")?)?;
    assert_eq!(read(&skill, SKILL_FILE)?, "behind\n");

    // A branch replaced by one whose name extends it does not block the
    // branches from being fetched again.
    upstream.git(&["update-ref", "-d", "refs/heads/release"])?;
    upstream.git(&["switch", "--quiet", "--create", "release/v2"])?;
    upstream.write("skills/a/SKILL.md", "release\n")?;
    let release = upstream.commit("release")?;
    upstream.git(&["switch", "--quiet", "main"])?;
    upstream.write("skills/a/SKILL.md", "after release\n")?;
    upstream.commit("after release")?;
    let skill = cache.extract(&source_name, &source, &release, &path("skills/a")?)?;
    assert_eq!(read(&skill, SKILL_FILE)?, "release\n");

    let never_existed = Revision::new(&"0".repeat(40))?;
    for missing in [&rewritten, &never_existed] {
        let err = cache
            .extract(&source_name, &source, missing, &path("skills/a")?)
            .err()
            .ok_or("an unreachable Revision must fail")?;
        let SourceError::RevisionUnreachable {
            revision, detail, ..
        } = &err
        else {
            return Err(format!("expected RevisionUnreachable, got {err:?}").into());
        };
        assert_eq!(revision, missing);
        // The cache really spoke protocol v0.
        assert!(detail.contains("unadvertised object"), "{detail}");
    }
    Ok(())
}

#[test]
fn a_cached_revision_survives_an_upstream_rewrite() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "1\n")?;
    let first = upstream.commit("first")?;
    upstream.write("skills/a/SKILL.md", "pinned\n")?;
    let pinned = upstream.commit("pinned")?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    assert_eq!(cache.fetch(&source_name, &source)?, pinned);

    upstream.rewrite_to(&first)?;
    upstream.write("skills/a/SKILL.md", "rewritten\n")?;
    let rewritten = upstream.commit("rewritten")?;
    assert!(!upstream.has_commit(&pinned));
    assert_eq!(cache.fetch(&source_name, &source)?, rewritten);

    let old = cache.extract(&source_name, &source, &pinned, &path("skills/a")?)?;
    assert_eq!(read(&old, SKILL_FILE)?, "pinned\n");
    Ok(())
}

#[test]
fn cache_only_extraction_never_contacts_the_source() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "pinned\n")?;
    let pinned = upstream.commit("pinned")?;
    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    let skill = path("skills/a")?;

    // No cache yet: nothing is cloned.
    let err = cache
        .extract_cached(&source_name, &source, &pinned, &skill)
        .err()
        .ok_or("extraction without a cache succeeded")?;
    assert!(matches!(err, SourceError::NotCached { .. }), "{err:?}");
    assert!(err.to_string().contains("never fetches"));
    assert!(!cache.root().exists(), "no cache is created");

    assert_eq!(cache.fetch(&source_name, &source)?, pinned);
    upstream.write("skills/a/SKILL.md", "later\n")?;
    let later = upstream.commit("later")?;

    // A Revision the cache lacks is not fetched, even though the Source has
    // it.
    let err = cache
        .extract_cached(&source_name, &source, &later, &skill)
        .err()
        .ok_or("an uncached Revision was extracted")?;
    assert!(matches!(err, SourceError::NotCached { .. }), "{err:?}");

    // The cached Revision is extracted with the Source gone entirely.
    fs::remove_dir_all(&upstream.repository)?;
    let extracted = cache.extract_cached(&source_name, &source, &pinned, &skill)?;
    assert_eq!(read(&extracted, SKILL_FILE)?, "pinned\n");
    Ok(())
}

#[test]
fn a_fresh_cache_fetches_a_pinned_revision_by_identifier() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "pinned\n")?;
    let pinned = upstream.commit("pinned")?;
    // Reachable upstream only through a reference that is not a branch, so
    // cloning does not bring it into the cache.
    upstream.git(&["update-ref", "refs/keep/pinned", pinned.as_str()])?;
    upstream.git(&["checkout", "--quiet", "--orphan", "fresh"])?;
    upstream.write("skills/a/SKILL.md", "fresh\n")?;
    upstream.commit("fresh")?;
    upstream.git(&["branch", "--quiet", "--force", "main", "fresh"])?;
    upstream.git(&["switch", "--quiet", "main"])?;
    upstream.git(&["branch", "--quiet", "--delete", "--force", "fresh"])?;

    // Never fetched before: extraction clones, then fetches the Revision.
    let skill =
        sandbox
            .cache()
            .extract(&name("s")?, &upstream.source(), &pinned, &path("skills/a")?)?;
    assert_eq!(read(&skill, SKILL_FILE)?, "pinned\n");
    Ok(())
}

#[test]
fn a_missing_git_binary_has_a_clear_diagnostic() -> TestResult {
    let sandbox = Sandbox::new()?;
    let missing = sandbox.dir.path().join("bin/git");
    let cache = SourceCache::new(
        sandbox.dir.path().join("cache"),
        Git::with_program(&missing),
    );
    let source = Source {
        url: "https://example.com/skills.git".to_owned(),
        branch: None,
    };
    let err = cache
        .fetch(&name("s")?, &source)
        .err()
        .ok_or("fetching without git must fail")?;
    assert!(matches!(err, SourceError::GitMissing { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!(
            "system `git` was not found (looked for `{}`); install Git and make sure `git` is on PATH, since skill-curator fetches Sources with it",
            missing.display()
        )
    );

    let not_on_path = SourceCache::new(
        sandbox.dir.path().join("cache"),
        Git::with_program("skill-curator-no-such-git"),
    );
    let err = not_on_path
        .extract(
            &name("s")?,
            &source,
            &Revision::new(&"a".repeat(40))?,
            &path("skills/a")?,
        )
        .err()
        .ok_or("extracting without git must fail")?;
    assert!(matches!(err, SourceError::GitMissing { .. }), "{err:?}");
    Ok(())
}

#[test]
fn a_moved_upstream_path_fails_with_candidates() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/grill-me/SKILL.md", &skill_md("1"))?;
    upstream.write("skills/review/SKILL.md", "review\n")?;
    upstream.commit("first")?;
    upstream.git(&["mv", "skills/grill-me", "skills/engineering"])?;
    upstream.write("skills/engineering/grill-me/SKILL.md", &skill_md("2"))?;
    fs::remove_file(upstream.dir.join("skills/engineering/SKILL.md"))?;
    upstream.write("skills/notes/README.md", "not a Skill\n")?;
    let r_new = upstream.commit("move")?;

    let cache = sandbox.cache();
    let source_name = name("mattpocock")?;
    let source = upstream.source();
    assert_eq!(cache.fetch(&source_name, &source)?, r_new);

    let err = cache
        .extract(&source_name, &source, &r_new, &path("skills/grill-me")?)
        .err()
        .ok_or("a moved Skill must not resolve")?;
    let SourceError::SkillNotFound {
        reason, candidates, ..
    } = &err
    else {
        return Err(format!("expected SkillNotFound, got {err:?}").into());
    };
    assert_eq!(reason, &NotFoundReason::Missing);
    assert_eq!(
        candidates,
        &[path("skills/engineering/grill-me")?, path("skills/review")?]
    );
    assert_eq!(
        err.to_string(),
        format!(
            "Source `mattpocock` has no Skill at `skills/grill-me` at Revision {r_new}: the path does not exist; set `upstream_path` in manifest.toml to the Skill's current directory. Skill directories found at that Revision:\n  - skills/engineering/grill-me\n  - skills/review"
        )
    );

    let err = cache
        .extract(&source_name, &source, &r_new, &path("skills/notes")?)
        .err()
        .ok_or("a directory without SKILL.md is not a Skill")?;
    assert!(
        matches!(
            err,
            SourceError::SkillNotFound {
                reason: NotFoundReason::NoSkillFile,
                ..
            }
        ),
        "{err:?}"
    );
    Ok(())
}

#[test]
fn symbolic_links_in_a_skill_are_refused() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("secret.txt", "do not leak\n")?;
    upstream.write("skills/evil/SKILL.md", "evil\n")?;
    symlink("../../secret.txt", upstream.dir.join("skills/evil/outside"))?;
    symlink("/etc/passwd", upstream.dir.join("skills/evil/absolute"))?;
    symlink("SKILL.md", upstream.dir.join("skills/evil/inside"))?;
    let revision = upstream.commit("links")?;

    let err = sandbox
        .cache()
        .extract(
            &name("s")?,
            &upstream.source(),
            &revision,
            &path("skills/evil")?,
        )
        .err()
        .ok_or("symbolic links must be refused")?;
    assert_eq!(
        refused_entries(&err)?,
        [
            (
                "absolute",
                EntryProblem::Symlink {
                    target: "/etc/passwd".to_owned(),
                    escapes: true
                }
            ),
            (
                "inside",
                EntryProblem::Symlink {
                    target: "SKILL.md".to_owned(),
                    escapes: false
                }
            ),
            (
                "outside",
                EntryProblem::Symlink {
                    target: "../../secret.txt".to_owned(),
                    escapes: true
                }
            ),
        ]
    );
    let message = err.to_string();
    assert!(
        message.contains(
            "\"outside\": symbolic link to \"../../secret.txt\", which points outside the Skill directory"
        ),
        "{message}"
    );
    Ok(())
}

#[test]
fn traversal_and_git_metadata_entries_are_refused() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("seed", "x\n")?;
    let blob = upstream.git(&["hash-object", "-w", "seed"])?;
    // Trees `git add` cannot produce but a hostile repository can hold.
    let git_dir =
        upstream.git_with_input(&["mktree"], Some(&format!("100644 blob {blob}\tconfig\n")))?;
    let skill = upstream.git_with_input(
        &["mktree"],
        Some(&format!(
            "100644 blob {blob}\t..\n040000 tree {git_dir}\t.git\n100644 blob {blob}\tSKILL.md\n"
        )),
    )?;
    let skills = upstream.git_with_input(
        &["mktree"],
        Some(&format!("040000 tree {skill}\thostile\n")),
    )?;
    let root = upstream.git_with_input(
        &["mktree"],
        Some(&format!("040000 tree {skills}\tskills\n")),
    )?;
    let commit = upstream.git(&["commit-tree", "-m", "hostile", &root])?;
    upstream.git(&["update-ref", "refs/heads/main", &commit])?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    let revision = cache.fetch(&source_name, &source)?;
    assert_eq!(revision.as_str(), commit);
    let err = cache
        .extract(&source_name, &source, &revision, &path("skills/hostile")?)
        .err()
        .ok_or("hostile paths must be refused")?;
    assert_eq!(
        refused_entries(&err)?,
        [
            ("..", EntryProblem::InvalidPath(PathError::Traversal)),
            (".git/config", EntryProblem::InvalidPath(PathError::GitDir)),
        ]
    );
    Ok(())
}

#[test]
fn detects_root_and_subdirectory_licenses() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("plugins")?;
    upstream.write("LICENSE", MIT)?;
    upstream.write("pstack/LICENSE", APACHE)?;
    upstream.write("pstack/critique/SKILL.md", "critique\n")?;
    upstream.write("pstack/critique/COPYING.txt", "Ask the author first.\n")?;
    upstream.write("unrelated/LICENSE", MIT)?;
    let revision = upstream.commit("licensed")?;

    let source_name = name("pstack")?;
    let skill = sandbox.cache().extract(
        &source_name,
        &upstream.source(),
        &revision,
        &path("pstack/critique")?,
    )?;
    let licenses = skill
        .licenses()
        .iter()
        .map(|license| (license.path.as_str(), license.summary.as_deref()))
        .collect::<Vec<_>>();
    assert_eq!(
        licenses,
        [
            ("LICENSE", Some("MIT")),
            ("pstack/LICENSE", Some("Apache-2.0")),
            ("pstack/critique/COPYING.txt", None),
        ]
    );
    let warning = skill
        .license_warning()
        .ok_or("an unclear license must warn")?;
    assert_eq!(
        warning.kind,
        LicenseWarningKind::Unclear {
            paths: vec![path("pstack/critique/COPYING.txt")?]
        }
    );

    let provenance = skill.provenance(Some(name("critique")?));
    assert_eq!(provenance.source, source_name);
    assert_eq!(provenance.url, upstream.source().url);
    assert_eq!(provenance.upstream_path, path("pstack/critique")?);
    assert_eq!(provenance.revision, revision);
    assert_eq!(provenance.licenses, skill.licenses());
    Ok(())
}

#[test]
fn warns_explicitly_when_no_license_is_found() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "a\n")?;
    upstream.write("other/LICENSE", MIT)?;
    let revision = upstream.commit("unlicensed")?;

    let skill = sandbox.cache().extract(
        &name("s")?,
        &upstream.source(),
        &revision,
        &path("skills/a")?,
    )?;
    assert!(skill.licenses().is_empty());
    let warning = skill
        .license_warning()
        .ok_or("a missing license must warn")?;
    assert_eq!(warning.kind, LicenseWarningKind::Missing);
    assert!(
        warning
            .to_string()
            .starts_with("no license file found for Skill `skills/a` of Source `s`"),
        "{warning}"
    );
    Ok(())
}

#[test]
fn hooks_in_the_cache_never_run() -> TestResult {
    let sandbox = Sandbox::new()?;
    let upstream = sandbox.upstream("upstream")?;
    upstream.write("skills/a/SKILL.md", "1\n")?;
    upstream.commit("first")?;

    let cache = sandbox.cache();
    let source_name = name("s")?;
    let source = upstream.source();
    cache.fetch(&source_name, &source)?;

    let repository = fs::read_dir(cache.root().join("sources"))?
        .next()
        .ok_or("no cache repository")??
        .path();
    assert!(
        !repository.join("hooks").exists(),
        "no template hooks are installed"
    );
    let marker = sandbox.dir.path().join("hook-ran");
    fs::create_dir(repository.join("hooks"))?;
    for hook in ["reference-transaction", "post-checkout", "pre-auto-gc"] {
        let script = repository.join("hooks").join(hook);
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    }

    upstream.write("skills/a/SKILL.md", "2\n")?;
    let revision = upstream.commit("second")?;
    assert_eq!(cache.fetch(&source_name, &source)?, revision);
    cache.extract(&source_name, &source, &revision, &path("skills/a")?)?;
    assert!(!marker.exists(), "a hook ran");
    Ok(())
}

#[test]
fn extracted_provenance_feeds_the_lock_and_attribution() -> TestResult {
    let sandbox = Sandbox::new()?;
    let skills = sandbox.upstream("skills")?;
    skills.write("LICENSE", MIT)?;
    skills.write("skills/review/SKILL.md", "review\n")?;
    let review_revision = skills.commit("review")?;
    let plugins = sandbox.upstream("plugins")?;
    plugins.write("pstack/LICENSE", APACHE)?;
    plugins.write("pstack/critique/SKILL.md", "critique\n")?;
    let critique_revision = plugins.commit("critique")?;

    let cache = sandbox.cache();
    let review = cache.extract(
        &name("mattpocock")?,
        &skills.source(),
        &review_revision,
        &path("skills/review")?,
    )?;
    let critique = cache.extract(
        &name("pstack")?,
        &plugins.source(),
        &critique_revision,
        &path("pstack/critique")?,
    )?;

    let mut lock = Lock::default();
    lock.skills.insert(
        name("review")?,
        LockedSkill {
            output_hash: tree_digest(review.path())?,
            inputs_hash: None,
            provenance: vec![review.provenance(None)],
            applied: None,
        },
    );
    lock.skills.insert(
        name("code-review")?,
        LockedSkill {
            output_hash: tree_digest(critique.path())?,
            inputs_hash: None,
            provenance: vec![
                critique.provenance(Some(name("critique")?)),
                review.provenance(Some(name("review")?)),
            ],
            applied: None,
        },
    );
    let reparsed = Lock::from_toml(&lock.to_toml()?)?;
    assert_eq!(reparsed, lock);

    let text = attribution::render(&reparsed);
    let expected_lines = [
        "## `code-review`".to_owned(),
        "Composition of 2 upstream Skills.".to_owned(),
        format!(
            "- Input `critique`: `pstack/critique` from Source `pstack` (`{}`) at Revision `{critique_revision}`",
            plugins.source().url
        ),
        "  - License: `pstack/LICENSE` (`Apache-2.0`)".to_owned(),
        format!(
            "- Input `review`: `skills/review` from Source `mattpocock` (`{}`) at Revision `{review_revision}`",
            skills.source().url
        ),
        "## `review`".to_owned(),
        format!(
            "- `skills/review` from Source `mattpocock` (`{}`) at Revision `{review_revision}`",
            skills.source().url
        ),
        "  - License: `LICENSE` (`MIT`)".to_owned(),
    ];
    let lines = text.lines().collect::<Vec<_>>();
    let mut position = 0;
    for expected in &expected_lines {
        position += lines[position..]
            .iter()
            .position(|line| line == expected)
            .ok_or_else(|| format!("missing or out of order: {expected}\n{text}"))?;
    }
    assert_eq!(attribution::render(&reparsed), text);
    Ok(())
}
