//! `skill-curator materialize` against a Collection whose Skills were
//! accepted from a local fixture Git repository. The command runs with an
//! isolated home, Source cache, and Git configuration; most tests delete the
//! upstream repository first, which proves `materialize` never fetches.

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::assert::Assert;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use skill_curator::adaptation::{Rules, SkillTree, det};
use skill_curator::collection::Collection;
use skill_curator::collection::lock::{Lock, LockedSkill, Provenance};
use skill_curator::collection::manifest::Skill;
use skill_curator::hash::tree_digest;
use skill_curator::safety::{HomeDir, Name, RelativePath};
use skill_curator::source::{Git, SourceCache};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

/// Exit code for failures, including Skills left needing attention.
const FAILURE: i32 = 1;

const UPSTREAM_SKILL_MD: &str = "---
name: grill-me
description: Grill the user.
icon: flame
version: 1
---
# Grill me

Run this first:

```!
git status --short
```

---

Ask one question at a time.

***
";

/// The Manifest. `grilling` is a deterministic Adapt Skill; `voiced` and the
/// Composition `paired` are agent-assisted, so their accepted files are an
/// agent rewrite.
const MANIFEST: &str = r#"[sources.upstream]
url = "URL"

[skills.grill-me]
mode = "copy"
source = "upstream"
upstream_path = "skills/grill-me"

[skills.grilling]
mode = "adapt"
source = "upstream"
upstream_path = "skills/grill-me"
frontmatter = { remove = ["icon"] }
files = { exclude = ["evals"] }

[skills.voiced]
mode = "adapt"
source = "upstream"
upstream_path = "skills/grill-me"
intent = "intents/voiced.md"
frontmatter = { remove = ["icon"] }
files = { exclude = ["evals"] }

[skills.paired]
mode = "composition"
intent = "intents/paired.md"
frontmatter = { set = { name = "paired" } }

[skills.paired.inputs.base]
source = "upstream"
upstream_path = "skills/grill-me"

[skills.paired.inputs.partner]
source = "upstream"
upstream_path = "skills/partner"

[skills.notes]
mode = "local"
"#;

/// The rules of `grilling` and `voiced`, as written in [`MANIFEST`].
const GRILLING_RULES: &str = "upstream_path = \"skills/grill-me\"\nfrontmatter = { remove = [\"icon\"] }\nfiles = { exclude = [\"evals\"] }";
const VOICED_RULES: &str = "intent = \"intents/voiced.md\"\nfrontmatter = { remove = [\"icon\"] }\nfiles = { exclude = [\"evals\"] }";

/// Line the simulated agent rewrite adds to `voiced`.
const AGENT_LINE: &str = "Answer in the user's voice.\n";

/// A Collection accepted from an upstream fixture repository, with its own
/// home, Source cache, and Git configuration.
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let fixture = Self {
            dir: TempDir::new()?,
        };
        fs::create_dir(fixture.home())?;
        fixture.create_upstream()?;
        fixture.create_collection()?;
        fixture.accept_all()?;
        Ok(fixture)
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn cache_home(&self) -> PathBuf {
        self.dir.path().join("cache-home")
    }

    fn repository(&self) -> PathBuf {
        self.dir.path().join("upstream.git")
    }

    fn collection(&self) -> PathBuf {
        self.dir.path().join("collection")
    }

    fn skill(&self, name: &str) -> PathBuf {
        self.collection().join("skills").join(name)
    }

    fn env(&self) -> Vec<(&'static str, OsString)> {
        vec![
            ("HOME", self.home().into_os_string()),
            ("XDG_CACHE_HOME", self.cache_home().into_os_string()),
            ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
        ]
    }

    fn git(&self, work_tree: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
        let output = Command::new("git")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .envs(self.env())
            .env("GIT_AUTHOR_NAME", "Upstream")
            .env("GIT_AUTHOR_EMAIL", "upstream@example.com")
            .env("GIT_COMMITTER_NAME", "Upstream")
            .env("GIT_COMMITTER_EMAIL", "upstream@example.com")
            .current_dir(work_tree)
            .args(args)
            .output()?;
        finish(&output, args)
    }

    fn create_upstream(&self) -> TestResult {
        let work = self.dir.path().join("upstream");
        let skill = work.join("skills/grill-me");
        fs::create_dir_all(skill.join("evals"))?;
        fs::create_dir_all(skill.join("scripts"))?;
        fs::create_dir_all(skill.join("references"))?;
        fs::write(skill.join("SKILL.md"), UPSTREAM_SKILL_MD)?;
        fs::create_dir_all(work.join("skills/partner"))?;
        fs::write(
            work.join("skills/partner/SKILL.md"),
            "---\nname: partner\ndescription: Partner.\n---\nPartner.\n",
        )?;
        fs::write(skill.join("README.md"), "Upstream readme.\n")?;
        fs::write(skill.join("evals/basic.md"), "Eval.\n")?;
        fs::write(skill.join("references/guide.md"), "Guide.\n")?;
        fs::write(skill.join("scripts/run.sh"), "#!/bin/sh\necho run\n")?;
        fs::set_permissions(
            skill.join("scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )?;
        let repository = self.repository();
        let repository = repository.to_str().ok_or("non-UTF-8 path")?;
        self.git(
            self.dir.path(),
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                repository,
            ],
        )?;
        let git_dir = format!("--git-dir={repository}");
        let work_tree = format!("--work-tree={}", work.display());
        self.git(&work, &[&git_dir, &work_tree, "add", "--all"])?;
        self.git(
            &work,
            &[&git_dir, &work_tree, "commit", "--quiet", "-m", "Initial"],
        )?;
        Ok(())
    }

    fn create_collection(&self) -> TestResult {
        let collection = self.collection();
        fs::create_dir_all(collection.join("intents"))?;
        fs::create_dir_all(collection.join("skills/notes"))?;
        fs::write(
            collection.join("intents/voiced.md"),
            "Use the user's voice.\n",
        )?;
        fs::write(collection.join("intents/paired.md"), "Pair the user.\n")?;
        fs::write(
            collection.join("skills/notes/SKILL.md"),
            "---\nname: notes\ndescription: Notes.\n---\nLocal.\n",
        )?;
        let url = format!("file://{}", self.repository().display());
        fs::write(
            collection.join("manifest.toml"),
            MANIFEST.replace("URL", &url),
        )?;
        Ok(())
    }

    fn cache(&self) -> Result<SourceCache, Box<dyn Error>> {
        let home = HomeDir::new(self.home())?;
        let root = SourceCache::default_root(Some(self.cache_home().as_os_str()), &home);
        let git = self
            .env()
            .into_iter()
            .fold(Git::system(), |git, (key, value)| git.env(key, value));
        Ok(SourceCache::new(root, git))
    }

    /// Accepts every upstream Skill as `update` would: fetches the Source,
    /// writes `det(R)` (plus an agent rewrite for `voiced`), and records the
    /// accepted state in `lock.toml`.
    fn accept_all(&self) -> TestResult {
        let collection = Collection::load(&self.collection())?;
        let cache = self.cache()?;
        let source_name = name("upstream")?;
        let source = collection
            .manifest()
            .sources
            .get(&source_name)
            .ok_or("no source")?
            .clone();
        let revision = cache.fetch(&source_name, &source)?;
        let upstream_path = RelativePath::new("skills/grill-me")?;
        let extracted = cache.extract(&source_name, &source, &revision, &upstream_path)?;
        let upstream = SkillTree::read(extracted.path())?;
        let mut lock = Lock::default();
        for (skill_name, skill) in &collection.manifest().skills {
            if matches!(skill, Skill::Local(_)) {
                lock.skills.insert(
                    skill_name.clone(),
                    LockedSkill {
                        output_hash: collection.output_digest(skill_name)?,
                        inputs_hash: None,
                        provenance: Vec::new(),
                        applied: None,
                    },
                );
                continue;
            }
            let mut tree = det(&upstream, &Rules::of(collection.manifest(), skill))?;
            let inputs = match skill {
                Skill::Composition(composition) => composition
                    .inputs
                    .iter()
                    .map(|(input, declared)| (Some(input.clone()), declared.upstream_path.clone()))
                    .collect(),
                _ => vec![(None, upstream_path.clone())],
            };
            if skill.intent().is_some() {
                let text = format!("{}{AGENT_LINE}", tree.skill_md()?);
                tree.set_skill_md(text);
            }
            tree.write_new(&collection.skill_dir(skill_name))?;
            lock.skills.insert(
                skill_name.clone(),
                LockedSkill {
                    output_hash: collection.output_digest(skill_name)?,
                    inputs_hash: collection.inputs_digest(skill_name)?,
                    provenance: inputs
                        .into_iter()
                        .map(|(input, upstream_path)| Provenance {
                            input,
                            source: source_name.clone(),
                            url: source.url.clone(),
                            upstream_path,
                            revision: revision.clone(),
                            licenses: Vec::new(),
                        })
                        .collect(),
                    applied: collection.applied_inputs(skill_name)?,
                },
            );
        }
        lock.write(&self.collection())?;
        Ok(())
    }

    /// Deletes the upstream repository: any fetch would now fail.
    fn delete_upstream(&self) -> TestResult {
        fs::remove_dir_all(self.repository())?;
        Ok(())
    }

    fn edit_manifest(&self, from: &str, to: &str) -> TestResult {
        let path = self.collection().join("manifest.toml");
        let text = fs::read_to_string(&path)?;
        if !text.contains(from) {
            return Err(format!("manifest.toml does not contain {from:?}").into());
        }
        fs::write(path, text.replacen(from, to, 1))?;
        Ok(())
    }

    fn materialize(&self, args: &[&str]) -> Assert {
        let mut command = cargo_bin_cmd!("skill-curator");
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command
            .envs(self.env())
            .current_dir(self.dir.path())
            .arg("materialize")
            .arg("--collection")
            .arg(self.collection())
            .args(args)
            .assert()
    }

    fn lock(&self) -> Result<Lock, Box<dyn Error>> {
        Ok(Lock::from_toml(&fs::read_to_string(
            self.collection().join("lock.toml"),
        )?)?)
    }

    fn lock_text(&self) -> Result<String, Box<dyn Error>> {
        Ok(fs::read_to_string(self.collection().join("lock.toml"))?)
    }

    fn read(&self, skill: &str, relative: &str) -> Result<String, Box<dyn Error>> {
        Ok(fs::read_to_string(self.skill(skill).join(relative))?)
    }

    /// Checks that `lock.toml` records the current files and inputs of
    /// `skill`, as a fresh hash computes them.
    fn assert_lock_matches(&self, skill: &str) -> TestResult {
        let collection = Collection::load(&self.collection())?;
        let skill_name = name(skill)?;
        let lock = self.lock()?;
        let locked = lock.skills.get(&skill_name).ok_or("not locked")?;
        assert_eq!(locked.output_hash, tree_digest(&self.skill(skill))?);
        assert_eq!(locked.inputs_hash, collection.inputs_digest(&skill_name)?);
        assert_eq!(locked.applied, collection.applied_inputs(&skill_name)?);
        Ok(())
    }
}

fn finish(output: &Output, args: &[&str]) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

fn name(raw: &str) -> Result<Name, Box<dyn Error>> {
    Ok(Name::new(raw)?)
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
                found.push(
                    entry
                        .path()
                        .strip_prefix(dir)?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The upstream `SKILL.md` body, which every D1 replay keeps verbatim.
fn upstream_body() -> Result<&'static str, Box<dyn Error>> {
    let (_, body) = UPSTREAM_SKILL_MD
        .split_once("version: 1\n---\n")
        .ok_or("no frontmatter")?;
    Ok(body)
}

#[test]
fn an_unchanged_collection_records_local_skills_and_then_reports_nothing() -> TestResult {
    let fixture = Fixture::new()?;
    let mut lock = fixture.lock()?;
    lock.skills.remove(&name("notes")?);
    lock.write(&fixture.collection())?;
    fixture.delete_upstream()?;
    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("grilling: unchanged"))
        .stdout(predicate::str::contains("voiced: unchanged"))
        .stdout(predicate::str::contains(
            "notes: recorded the local Skill in lock.toml",
        ));
    let lock = fixture.lock()?;
    let notes = lock.skills.get(&name("notes")?).ok_or("notes not locked")?;
    assert_eq!(notes.output_hash, tree_digest(&fixture.skill("notes"))?);
    assert!(notes.provenance.is_empty() && notes.inputs_hash.is_none());

    let before = fixture.lock_text()?;
    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("notes: unchanged"));
    assert_eq!(fixture.lock_text()?, before);
    Ok(())
}

#[test]
fn frontmatter_rule_changes_are_replayed_without_fetching() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace(
            "remove = [\"icon\"] }",
            "remove = [\"icon\", \"version\"], set = { disable-model-invocation = true } }",
        ),
    )?;
    fixture.edit_manifest(
        VOICED_RULES,
        &VOICED_RULES.replace(
            "remove = [\"icon\"] }",
            "remove = [], set = { description = \"Grill me, in my voice.\" } }",
        ),
    )?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("grilling: rebuilt"))
        .stdout(predicate::str::contains(
            "voiced: replayed the frontmatter rules (D1)",
        ));

    assert_eq!(
        fixture.read("grilling", "SKILL.md")?,
        format!(
            "---\nname: grill-me\ndescription: Grill the user.\ndisable-model-invocation: true\n---\n{}",
            upstream_body()?
        )
    );
    // The agent rewrite is kept; `icon` is no longer removed, so the replay
    // restores it from the pinned Revision.
    assert_eq!(
        fixture.read("voiced", "SKILL.md")?,
        format!(
            "---\nname: grill-me\ndescription: \"Grill me, in my voice.\"\nversion: 1\nicon: flame\n---\n{}{AGENT_LINE}",
            upstream_body()?
        )
    );
    fixture.assert_lock_matches("grilling")?;
    fixture.assert_lock_matches("voiced")?;

    // Replaying is deterministic: a second run finds nothing to do.
    let before = fixture.lock_text()?;
    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("grilling: unchanged"))
        .stdout(predicate::str::contains("voiced: unchanged"));
    assert_eq!(fixture.lock_text()?, before);
    Ok(())
}

#[test]
fn new_file_exclusions_are_replayed_without_fetching() -> TestResult {
    let fixture = Fixture::new()?;
    for rules in [GRILLING_RULES, VOICED_RULES] {
        fixture.edit_manifest(
            rules,
            &rules.replace(
                "exclude = [\"evals\"]",
                "exclude = [\"evals\", \"scripts/\"]",
            ),
        )?;
    }
    let voiced_skill_md = fixture.read("voiced", "SKILL.md")?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("voiced: removed `scripts/run.sh`"));

    let kept = ["README.md", "SKILL.md", "references/guide.md"];
    assert_eq!(files(&fixture.skill("grilling"))?, kept);
    assert_eq!(files(&fixture.skill("voiced"))?, kept);
    assert_eq!(fixture.read("voiced", "SKILL.md")?, voiced_skill_md);
    fixture.assert_lock_matches("grilling")?;
    fixture.assert_lock_matches("voiced")?;
    assert!(
        fs::read_dir(fixture.collection().join("skills"))?
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with('.')),
        "a staging directory was left in skills/"
    );
    Ok(())
}

#[test]
fn files_newly_kept_by_a_deterministic_skill_come_from_the_pinned_revision() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace("exclude = [\"evals\"]", "exclude = []"),
    )?;
    fixture.delete_upstream()?;

    fixture.materialize(&[]).success();
    assert_eq!(fixture.read("grilling", "evals/basic.md")?, "Eval.\n");
    let mode = fs::metadata(fixture.skill("grilling").join("scripts/run.sh"))?
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111, "the executable bit is kept");
    fixture.assert_lock_matches("grilling")?;
    Ok(())
}

#[test]
fn agent_assisted_changes_are_reported_and_leave_the_skill_alone() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit_manifest(
        VOICED_RULES,
        &VOICED_RULES.replace("exclude = [\"evals\"]", "exclude = []"),
    )?;
    let before = files(&fixture.skill("voiced"))?;
    let lock = fixture.lock_text()?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "voiced: needs an agent rewrite: the file rules (D2) now keep files the agent rewrite has not seen: `evals/basic.md`",
        ))
        .stdout(predicate::str::contains("--reset voiced"))
        .stderr(predicate::str::contains("1 Skill outcome needs attention"));
    assert_eq!(files(&fixture.skill("voiced"))?, before);
    assert_eq!(fixture.lock_text()?, lock);

    // An edited written intent needs an agent rewrite too.
    fixture.edit_manifest(
        &VOICED_RULES.replace("exclude = [\"evals\"]", "exclude = []"),
        VOICED_RULES,
    )?;
    fs::write(
        fixture.collection().join("intents/voiced.md"),
        "Use the user's voice, briefly.\n",
    )?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "voiced: needs an agent rewrite: the written intent changed",
        ));
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn manual_edits_are_detected_and_never_overwritten() -> TestResult {
    let fixture = Fixture::new()?;
    let edited = format!("{}Manual note.\n", fixture.read("grilling", "SKILL.md")?);
    fs::write(fixture.skill("grilling").join("SKILL.md"), &edited)?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace("remove = [\"icon\"]", "remove = [\"icon\", \"version\"]"),
    )?;
    let lock = fixture.lock_text()?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: manual edits in skills/grilling/",
        ))
        .stdout(predicate::str::contains("materialize --accept grilling"))
        .stdout(predicate::str::contains("materialize --reset grilling"));
    assert_eq!(fixture.read("grilling", "SKILL.md")?, edited);
    assert_eq!(fixture.lock_text()?, lock);

    // A deleted Skill directory is drift too, never silently rebuilt.
    fs::remove_dir_all(fixture.skill("grill-me"))?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grill-me: manual edits in skills/grill-me/ (the directory is missing)",
        ));
    assert!(!fixture.skill("grill-me").exists());
    Ok(())
}

#[test]
fn accept_records_manual_edits_and_then_replays_on_top_of_them() -> TestResult {
    let fixture = Fixture::new()?;
    let edited = format!("{}Manual note.\n", fixture.read("voiced", "SKILL.md")?);
    fs::write(fixture.skill("voiced").join("SKILL.md"), &edited)?;
    fs::write(fixture.skill("voiced").join("NOTES.md"), "Mine.\n")?;
    fixture.edit_manifest(
        VOICED_RULES,
        &VOICED_RULES.replace(
            "exclude = [\"evals\"]",
            "exclude = [\"evals\", \"references\"]",
        ),
    )?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&["--accept", "voiced"])
        .success()
        .stdout(predicate::str::contains(
            "voiced: accepted the manual edits in skills/voiced/ into lock.toml; update the written intent `intents/voiced.md`",
        ))
        .stdout(predicate::str::contains("warning").not())
        .stdout(predicate::str::contains("voiced: removed `references/guide.md`"));
    assert_eq!(fixture.read("voiced", "SKILL.md")?, edited);
    assert_eq!(fixture.read("voiced", "NOTES.md")?, "Mine.\n");
    assert!(!fixture.skill("voiced").join("references").exists());
    fixture.assert_lock_matches("voiced")?;

    fixture
        .materialize(&["--accept", "voiced"])
        .success()
        .stdout(predicate::str::contains(
            "voiced: no manual edits to accept",
        ));
    Ok(())
}

#[test]
fn reset_discards_manual_edits_without_fetching() -> TestResult {
    let fixture = Fixture::new()?;
    let accepted = fixture.read("grilling", "SKILL.md")?;
    let accepted_files = files(&fixture.skill("grilling"))?;
    fs::write(fixture.skill("grilling").join("SKILL.md"), "broken")?;
    fs::write(fixture.skill("grilling").join("stray.md"), "Stray.\n")?;
    fs::write(fixture.skill("voiced").join("README.md"), "Edited.\n")?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&["--reset", "grilling", "--reset", "voiced"])
        .success()
        .stdout(predicate::str::contains(
            "grilling: reset skills/grilling/ to its deterministic adaptation at Revision",
        ))
        .stdout(predicate::str::contains(
            "the written intent `intents/voiced.md` is no longer applied",
        ));
    assert_eq!(fixture.read("grilling", "SKILL.md")?, accepted);
    assert_eq!(files(&fixture.skill("grilling"))?, accepted_files);
    fixture.assert_lock_matches("grilling")?;
    assert_eq!(fixture.read("voiced", "README.md")?, "Upstream readme.\n");
    assert!(!fixture.read("voiced", "SKILL.md")?.contains(AGENT_LINE));

    // The reset `voiced` lacks its agent rewrite until one runs.
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains("grilling: unchanged"))
        .stdout(predicate::str::contains(
            "voiced: needs an agent rewrite: the written intent has not been applied yet",
        ));
    Ok(())
}

#[test]
fn symbolic_links_in_an_accepted_skill_are_never_followed() -> TestResult {
    let fixture = Fixture::new()?;
    let outside = fixture.dir.path().join("outside.md");
    fs::write(&outside, "Outside the Collection.\n")?;
    let readme = fixture.skill("grilling").join("README.md");
    fs::remove_file(&readme)?;
    symlink(&outside, &readme)?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains("grilling: manual edits"))
        .stdout(predicate::str::contains("symbolic link"));
    fixture
        .materialize(&["--accept", "grilling"])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: failed: Skill `grilling` has no manual edits to accept",
        ));

    fixture.delete_upstream()?;
    fixture.materialize(&["--reset", "grilling"]).success();
    assert!(!fs::symlink_metadata(&readme)?.file_type().is_symlink());
    assert_eq!(fixture.read("grilling", "README.md")?, "Upstream readme.\n");
    assert_eq!(fs::read_to_string(&outside)?, "Outside the Collection.\n");
    Ok(())
}

#[test]
fn file_rules_that_drop_skill_md_fail_and_change_nothing() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace("exclude = [\"evals\"]", "exclude = [\"*.md\"]"),
    )?;
    let before = files(&fixture.skill("grilling"))?;
    let lock = fixture.lock_text()?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: failed: the file rules (D2) drop `SKILL.md`, which every Skill must keep: `exclude` pattern `*.md` selects it",
        ));
    assert_eq!(files(&fixture.skill("grilling"))?, before);
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn file_rules_with_traversal_or_invalid_globs_are_rejected() -> TestResult {
    for (pattern, messages) in [
        (
            "../secrets",
            [
                "\"../secrets\" is not a valid path pattern",
                "`..` component",
            ],
        ),
        (
            "refs/[a",
            [
                "skills.grilling.files.exclude",
                "`refs/[a` is not a valid glob",
            ],
        ),
        (
            "./evals",
            [
                "\"./evals\" is not a valid path pattern",
                "empty or `.` segments",
            ],
        ),
    ] {
        let fixture = Fixture::new()?;
        fixture.edit_manifest(
            GRILLING_RULES,
            &GRILLING_RULES.replace("exclude = [\"evals\"]", &format!("exclude = [{pattern:?}]")),
        )?;
        let lock = fixture.lock_text()?;
        fixture
            .materialize(&[])
            .code(FAILURE)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(messages[0]))
            .stderr(predicate::str::contains(messages[1]));
        assert_eq!(fixture.lock_text()?, lock);
    }
    Ok(())
}

#[test]
fn an_uncached_revision_is_an_error_not_a_fetch() -> TestResult {
    let fixture = Fixture::new()?;
    fs::remove_dir_all(fixture.cache_home())?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace(
            "exclude = [\"evals\"]",
            "exclude = [\"evals\", \"scripts\"]",
        ),
    )?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains("this command never fetches"));
    assert!(
        !fixture.cache_home().exists(),
        "materialize created a Source cache"
    );
    assert!(fixture.skill("grilling").join("scripts/run.sh").exists());
    Ok(())
}

#[test]
fn options_naming_unknown_or_unresettable_skills_are_refused() -> TestResult {
    let fixture = Fixture::new()?;
    let lock = fixture.lock_text()?;
    for (args, message) in [
        (vec!["--accept", "nope"], "Skill `nope` is not declared"),
        (
            vec!["--accept", "grilling", "--reset", "grilling"],
            "both `--accept` and `--reset`",
        ),
        (
            vec!["--reset", "notes"],
            "a local Skill has no deterministic",
        ),
        (
            vec!["--accept", "notes"],
            "Skill `notes` is a local Skill: `materialize` records its files in lock.toml on every run, so there is nothing to accept",
        ),
    ] {
        fixture
            .materialize(&args)
            .code(FAILURE)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(message));
    }
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn composition_frontmatter_changes_are_replayed_and_input_changes_need_an_agent() -> TestResult {
    let fixture = Fixture::new()?;
    let accepted = fixture.read("paired", "SKILL.md")?;
    assert!(accepted.starts_with("---\nname: paired\n"));
    fixture.edit_manifest(
        "frontmatter = { set = { name = \"paired\" } }",
        "frontmatter = { set = { name = \"paired\" }, remove = [\"version\"] }",
    )?;
    fixture.delete_upstream()?;
    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains(
            "paired: replayed the frontmatter rules (D1)",
        ));
    assert_eq!(
        fixture.read("paired", "SKILL.md")?,
        accepted.replacen("version: 1\n", "", 1)
    );
    fixture.assert_lock_matches("paired")?;

    let lock = fixture.lock_text()?;
    fixture.edit_manifest(
        "upstream_path = \"skills/grill-me\"\n\n[skills.paired.inputs.partner]",
        "upstream_path = \"skills/grill-me\"\nfiles = { exclude = [\"scripts\"] }\n\n[skills.paired.inputs.partner]",
    )?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "paired: needs an agent rewrite: the file rules (D2) of a Composition input changed; `materialize` does not run agents, and a composition Skill has no deterministic adaptation to reset to\n",
        ))
        .stdout(predicate::str::contains("--reset paired").not());
    fixture
        .materialize(&["--reset", "paired"])
        .code(FAILURE)
        .stderr(predicate::str::contains(
            "a composition Skill has no deterministic adaptation",
        ));
    assert_eq!(fixture.lock_text()?, lock);
    assert!(fixture.skill("paired").join("scripts/run.sh").exists());
    Ok(())
}

#[test]
fn removing_the_written_intent_of_an_agent_rewrite_needs_a_reset() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit_manifest("intent = \"intents/voiced.md\"\n", "")?;
    let lock = fixture.lock_text()?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "voiced: cannot replay the change: the written intent was removed",
        ))
        .stdout(predicate::str::contains("materialize --reset voiced"));
    assert_eq!(fixture.lock_text()?, lock);

    fixture.delete_upstream()?;
    fixture.materialize(&["--reset", "voiced"]).success();
    assert!(!fixture.read("voiced", "SKILL.md")?.contains(AGENT_LINE));
    fixture.assert_lock_matches("voiced")?;
    fixture.materialize(&[]).success();
    Ok(())
}

#[test]
fn include_globs_are_replayed_without_fetching() -> TestResult {
    let fixture = Fixture::new()?;
    for rules in [GRILLING_RULES, VOICED_RULES] {
        fixture.edit_manifest(
            rules,
            &rules.replace(
                "exclude = [\"evals\"]",
                "exclude = [\"evals\"], include = [\"SKILL.md\", \"references\"]",
            ),
        )?;
    }
    let voiced_skill_md = fixture.read("voiced", "SKILL.md")?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("grilling: rebuilt"))
        .stdout(predicate::str::contains(
            "voiced: removed `README.md`, `scripts/run.sh` at the pinned Revision",
        ));
    let kept = ["SKILL.md", "references/guide.md"];
    assert_eq!(files(&fixture.skill("grilling"))?, kept);
    assert_eq!(files(&fixture.skill("voiced"))?, kept);
    assert_eq!(fixture.read("voiced", "SKILL.md")?, voiced_skill_md);
    fixture.assert_lock_matches("grilling")?;
    fixture.assert_lock_matches("voiced")?;
    let lock = fixture.lock()?;
    let applied = lock
        .skills
        .get(&name("grilling")?)
        .and_then(|locked| locked.applied.as_ref())
        .ok_or("no applied rules")?;
    let include = applied
        .files
        .iter()
        .find_map(|rules| rules.include.as_ref())
        .ok_or("include not recorded")?;
    assert_eq!(
        include.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        ["SKILL.md", "references"]
    );

    let before = fixture.lock_text()?;
    fixture
        .materialize(&[])
        .success()
        .stdout(predicate::str::contains("grilling: unchanged"))
        .stdout(predicate::str::contains("voiced: unchanged"));
    assert_eq!(fixture.lock_text()?, before);

    // An `include` list that leaves out `SKILL.md` fails and changes nothing.
    fixture.edit_manifest(
        "include = [\"SKILL.md\", \"references\"]",
        "include = [\"references\"]",
    )?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: failed: the file rules (D2) drop `SKILL.md`, which every Skill must keep: no `include` pattern selects it",
        ));
    assert_eq!(files(&fixture.skill("grilling"))?, kept);
    assert_eq!(fixture.lock_text()?, before);
    Ok(())
}

#[test]
fn reset_never_hides_a_changed_upstream_path() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace("skills/grill-me", "skills/partner"),
    )?;
    fs::write(fixture.skill("grilling").join("stray.md"), "Stray.\n")?;
    let before = files(&fixture.skill("grilling"))?;
    let lock = fixture.lock_text()?;
    fixture.delete_upstream()?;

    let needs_update = "grilling: the declared upstream Skill (Source or upstream path) differs from the one pinned in lock.toml; run `skill-curator update`";
    fixture
        .materialize(&["--reset", "grilling"])
        .code(FAILURE)
        .stdout(predicate::str::contains(needs_update))
        .stdout(predicate::str::contains("grilling: reset").not());
    assert_eq!(files(&fixture.skill("grilling"))?, before);
    assert_eq!(fixture.lock_text()?, lock);

    // The next run still asks for `update`, once the manual edit is gone.
    fs::remove_file(fixture.skill("grilling").join("stray.md"))?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(needs_update));
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn an_agent_assisted_skill_equal_to_its_deterministic_adaptation_still_needs_an_agent_for_new_files()
-> TestResult {
    let fixture = Fixture::new()?;
    // The agent rewrite of `voiced` changed nothing: its files are `det(R)`,
    // but its written intent is recorded as applied.
    let skill_md = fixture.read("voiced", "SKILL.md")?;
    let deterministic = skill_md.strip_suffix(AGENT_LINE).ok_or("no agent line")?;
    fs::write(fixture.skill("voiced").join("SKILL.md"), deterministic)?;
    fixture.materialize(&["--accept", "voiced"]).success();
    assert_eq!(
        fixture.read("voiced", "SKILL.md")?,
        fixture.read("grilling", "SKILL.md")?
    );

    fixture.edit_manifest(
        VOICED_RULES,
        &VOICED_RULES.replace("exclude = [\"evals\"]", "exclude = []"),
    )?;
    let before = files(&fixture.skill("voiced"))?;
    let lock = fixture.lock_text()?;
    fixture.delete_upstream()?;
    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "voiced: needs an agent rewrite: the file rules (D2) now keep files the agent rewrite has not seen: `evals/basic.md`",
        ));
    assert_eq!(files(&fixture.skill("voiced"))?, before);
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn accepting_edits_that_break_the_frontmatter_rules_warns() -> TestResult {
    let fixture = Fixture::new()?;
    // `grilling` removes `icon`; the edit brings it back.
    let edited = fixture.read("grilling", "SKILL.md")?.replacen(
        "version: 1\n",
        "version: 1\nicon: flame\n",
        1,
    );
    fs::write(fixture.skill("grilling").join("SKILL.md"), &edited)?;
    fixture.delete_upstream()?;
    fixture
        .materialize(&["--accept", "grilling"])
        .success()
        .stdout(predicate::str::contains(
            "grilling: accepted the manual edits in skills/grilling/ into lock.toml; warning: SKILL.md no longer satisfies the Skill's frontmatter rules (D1)",
        ))
        .stdout(predicate::str::contains("grilling: unchanged"));
    assert_eq!(fixture.read("grilling", "SKILL.md")?, edited);
    fixture.assert_lock_matches("grilling")?;
    Ok(())
}

#[test]
fn frontmatter_the_rules_cannot_edit_is_accepted_and_kept() -> TestResult {
    let fixture = Fixture::new()?;
    // Valid for many YAML readers, but not for D1: a plain scalar with `: `.
    let lax = |text: String| {
        text.replacen(
            "description: Grill the user.",
            "description: Use when: the user asks",
            1,
        )
    };
    let copy = lax(fixture.read("grill-me", "SKILL.md")?);
    fs::write(fixture.skill("grill-me").join("SKILL.md"), &copy)?;
    let adapted = lax(fixture.read("grilling", "SKILL.md")?);
    fs::write(fixture.skill("grilling").join("SKILL.md"), &adapted)?;
    fixture.delete_upstream()?;

    // A Copy Skill has no D1 rules: nothing to warn about.
    fixture
        .materialize(&["--accept", "grill-me", "--accept", "grilling"])
        .success()
        .stdout(predicate::str::contains(
            "grill-me: accepted the manual edits in skills/grill-me/ into lock.toml\n",
        ))
        .stdout(predicate::str::contains(
            "grilling: accepted the manual edits in skills/grilling/ into lock.toml; warning: SKILL.md no longer satisfies the Skill's frontmatter rules (D1)",
        ));
    assert_eq!(fixture.read("grill-me", "SKILL.md")?, copy);
    assert_eq!(fixture.read("grilling", "SKILL.md")?, adapted);
    fixture.assert_lock_matches("grill-me")?;
    fixture.assert_lock_matches("grilling")?;
    fixture.materialize(&[]).success();

    // A reset still rebuilds the Copy Skill from the pinned Revision.
    fixture.materialize(&["--reset", "grill-me"]).success();
    assert_eq!(fixture.read("grill-me", "SKILL.md")?, UPSTREAM_SKILL_MD);
    Ok(())
}

#[test]
fn manual_edits_on_a_composition_are_never_answered_with_a_reset() -> TestResult {
    let fixture = Fixture::new()?;
    let edited = format!("{}Manual note.\n", fixture.read("paired", "SKILL.md")?);
    fs::write(fixture.skill("paired").join("SKILL.md"), &edited)?;
    let lock = fixture.lock_text()?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "paired: manual edits in skills/paired/ (its files differ from the accepted output recorded in lock.toml); run `skill-curator materialize --accept paired` to keep them, or restore skills/paired/ from the Collection's Git history to discard them\n",
        ))
        .stdout(predicate::str::contains("--reset paired").not());
    assert_eq!(fixture.read("paired", "SKILL.md")?, edited);
    assert_eq!(fixture.lock_text()?, lock);

    // The suggested `--accept` works.
    fixture.materialize(&["--accept", "paired"]).success();
    assert_eq!(fixture.read("paired", "SKILL.md")?, edited);
    fixture.assert_lock_matches("paired")?;

    // A missing directory has nothing to accept; the way back is Git.
    fs::remove_dir_all(fixture.skill("paired"))?;
    fixture
        .materialize(&["--accept", "paired"])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "paired: failed: Skill `paired` has no manual edits to accept: `skills/paired/` is missing; restore `skills/paired/` from the Collection's Git history\n",
        ))
        .stdout(predicate::str::contains("--reset paired").not());
    Ok(())
}

#[test]
fn a_newly_kept_file_never_overwrites_an_accepted_file_at_its_path() -> TestResult {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.skill("grilling").join("evals"))?;
    fs::write(fixture.skill("grilling").join("evals/basic.md"), "Mine.\n")?;
    fixture.materialize(&["--accept", "grilling"]).success();
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace("exclude = [\"evals\"]", "exclude = []"),
    )?;
    let before = files(&fixture.skill("grilling"))?;
    let lock = fixture.lock_text()?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: cannot replay the change: the file rules (D2) now keep `evals/basic.md`, but the accepted Skill already has a file there; run `skill-curator materialize --reset grilling` to rebuild it\n",
        ));
    assert_eq!(fixture.read("grilling", "evals/basic.md")?, "Mine.\n");
    assert_eq!(files(&fixture.skill("grilling"))?, before);
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn a_lock_entry_without_applied_rules_blocks_the_replay() -> TestResult {
    let fixture = Fixture::new()?;
    let mut lock = fixture.lock()?;
    for skill in ["grilling", "paired"] {
        lock.skills
            .get_mut(&name(skill)?)
            .ok_or("not locked")?
            .applied = None;
    }
    lock.write(&fixture.collection())?;
    fixture.edit_manifest(
        GRILLING_RULES,
        &GRILLING_RULES.replace("remove = [\"icon\"]", "remove = [\"icon\", \"version\"]"),
    )?;
    fixture.edit_manifest(
        "frontmatter = { set = { name = \"paired\" } }",
        "frontmatter = { set = { name = \"paired\" }, remove = [\"version\"] }",
    )?;
    let grilling = fixture.read("grilling", "SKILL.md")?;
    let paired = fixture.read("paired", "SKILL.md")?;
    let lock = fixture.lock_text()?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: cannot replay the change: lock.toml does not record which rules produced the accepted Skill; run `skill-curator materialize --reset grilling` to rebuild it\n",
        ))
        .stdout(predicate::str::contains(
            "paired: cannot replay the change: lock.toml does not record which rules produced the accepted Skill; a composition Skill has no deterministic adaptation to reset to; run `skill-curator update` to accept it again\n",
        ));
    assert_eq!(fixture.read("grilling", "SKILL.md")?, grilling);
    assert_eq!(fixture.read("paired", "SKILL.md")?, paired);
    assert_eq!(fixture.lock_text()?, lock);

    // The suggested reset records the rules again.
    fixture.materialize(&["--reset", "grilling"]).code(FAILURE);
    assert!(!fixture.read("grilling", "SKILL.md")?.contains("version:"));
    fixture.assert_lock_matches("grilling")?;
    Ok(())
}

#[test]
fn a_symbolic_link_in_place_of_skills_is_never_written_through() -> TestResult {
    let fixture = Fixture::new()?;
    // Without a local Skill, loading the Collection never looks inside
    // `skills/`, so only the write path can refuse the link.
    fixture.edit_manifest("\n[skills.notes]\nmode = \"local\"\n", "")?;
    let outside = fixture.dir.path().join("outside");
    let skills = fixture.collection().join("skills");
    fs::rename(&skills, &outside)?;
    symlink(&outside, &skills)?;
    fs::write(outside.join("grilling/stray.md"), "Stray.\n")?;
    let before = files(&outside)?;
    let lock = fixture.lock_text()?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&["--reset", "grilling"])
        .code(FAILURE)
        .stdout(predicate::str::contains(format!(
            "grilling: failed: {} must be a directory of the Collection, not a symbolic link or another kind of file\n",
            skills.display()
        )));
    assert_eq!(files(&outside)?, before);
    assert_eq!(
        fs::read_to_string(outside.join("grilling/stray.md"))?,
        "Stray.\n"
    );
    assert!(
        fs::read_dir(&outside)?
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with('.')),
        "a staging directory was created through the link"
    );
    assert!(fs::symlink_metadata(&skills)?.file_type().is_symlink());
    assert_eq!(fixture.lock_text()?, lock);
    Ok(())
}

#[test]
fn a_symbolic_link_in_place_of_a_skill_is_replaced_not_followed() -> TestResult {
    let fixture = Fixture::new()?;
    let outside = fixture.dir.path().join("outside");
    let skill = fixture.skill("grilling");
    fs::rename(&skill, &outside)?;
    fs::write(outside.join("SKILL.md"), "Outside the Collection.\n")?;
    symlink(&outside, &skill)?;
    let before = files(&outside)?;
    fixture.delete_upstream()?;

    fixture
        .materialize(&[])
        .code(FAILURE)
        .stdout(predicate::str::contains(
            "grilling: manual edits in skills/grilling/",
        ));
    fixture
        .materialize(&["--reset", "grilling"])
        .success()
        .stdout(predicate::str::contains("grilling: reset skills/grilling/"));
    assert!(fs::symlink_metadata(&skill)?.file_type().is_dir());
    assert!(!fixture.read("grilling", "SKILL.md")?.contains("Outside"));
    fixture.assert_lock_matches("grilling")?;
    assert_eq!(files(&outside)?, before);
    assert_eq!(
        fs::read_to_string(outside.join("SKILL.md"))?,
        "Outside the Collection.\n"
    );
    Ok(())
}

#[test]
fn accepting_a_frontmatter_merge_key_warns() -> TestResult {
    let fixture = Fixture::new()?;
    // `grilling` removes `icon`; a merge-aware YAML reader reads it back
    // from the merge.
    let edited = fixture.read("grilling", "SKILL.md")?.replacen(
        "version: 1\n",
        "version: 1\n<<: {icon: flame}\n",
        1,
    );
    fs::write(fixture.skill("grilling").join("SKILL.md"), &edited)?;
    fixture.delete_upstream()?;
    fixture
        .materialize(&["--accept", "grilling"])
        .success()
        .stdout(predicate::str::contains(
            "grilling: accepted the manual edits in skills/grilling/ into lock.toml; warning: SKILL.md no longer satisfies the Skill's frontmatter rules (D1)",
        ));
    assert_eq!(fixture.read("grilling", "SKILL.md")?, edited);
    fixture.assert_lock_matches("grilling")?;
    Ok(())
}
