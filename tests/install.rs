//! `skill-curator install` against a Collection of local Skills and
//! Fragments, with an isolated home, configuration, and state directory.
//! Skills are accepted with `skill-curator materialize`, which needs no
//! Source, so these tests use neither Git nor the network.

use std::error::Error;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use assert_cmd::assert::Assert;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

/// Exit code for failures, including paths left needing attention.
const FAILURE: i32 = 1;

const HEADER: &str = "<!-- Managed by skill-curator -->";

const MANIFEST: &str = r#"[skills.grill-me]
mode = "local"

[skills.commit]
mode = "local"

[destinations.claude]
harness = "claude-code"
instructions = { file = "~/.claude/CLAUDE.md", fragments = ["fragments/git.md", "fragments/claude.md"] }

[destinations.pi]
harness = "pi"
frontmatter = { remove = ["disable-model-invocation"] }
instructions = { file = "~/.pi/agent/AGENTS.md", fragments = ["fragments/git.md"] }

[destinations.codex]
harness = "codex"
enabled = false
"#;

const GRILL_ME: &str = "---\nname: grill-me\ndescription: Grill the user.\n---\n# Grill me\n\nAsk one question at a time.\n";
const COMMIT: &str = "---\nname: commit\ndescription: Write a commit.\ndisable-model-invocation: true\n---\n# Commit\n";
const GIT_FRAGMENT: &str = "# Git\n\nUse Conventional Commits.\n";
const CLAUDE_FRAGMENT: &str = "Prefer `rg` over `grep`.";

/// A Collection with an isolated home, configuration, and state directory.
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let fixture = Self {
            dir: TempDir::new()?,
        };
        fs::create_dir(fixture.home())?;
        let collection = fixture.collection();
        fs::create_dir_all(collection.join("fragments"))?;
        fs::write(collection.join("manifest.toml"), MANIFEST)?;
        fixture.write_skill("grill-me", "SKILL.md", GRILL_ME)?;
        fixture.write_skill("grill-me", "references/guide.md", "Guide.\n")?;
        fixture.write_skill("commit", "SKILL.md", COMMIT)?;
        fs::write(collection.join("fragments/git.md"), GIT_FRAGMENT)?;
        fs::write(collection.join("fragments/claude.md"), CLAUDE_FRAGMENT)?;
        fixture.accept()?;
        Ok(fixture)
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn collection(&self) -> PathBuf {
        self.dir.path().join("collection")
    }

    fn config_home(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn state_home(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn state_file(&self) -> PathBuf {
        self.state_home().join("skill-curator/install.toml")
    }

    fn claude_skills(&self) -> PathBuf {
        self.home().join(".claude/skills")
    }

    fn pi_skills(&self) -> PathBuf {
        self.home().join(".pi/agent/skills")
    }

    fn codex_skills(&self) -> PathBuf {
        self.home().join(".agents/skills")
    }

    fn claude_md(&self) -> PathBuf {
        self.home().join(".claude/CLAUDE.md")
    }

    fn write_skill(&self, name: &str, relative: &str, contents: &str) -> TestResult {
        let path = self.collection().join("skills").join(name).join(relative);
        fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        fs::write(path, contents)?;
        Ok(())
    }

    fn write_manifest(&self, text: &str) -> TestResult {
        fs::write(self.collection().join("manifest.toml"), text)?;
        Ok(())
    }

    /// Writes `machine.toml` at its default location.
    fn write_machine(&self, text: &str) -> TestResult {
        let path = self.config_home().join("skill-curator/machine.toml");
        fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        fs::write(path, text)?;
        Ok(())
    }

    fn command(&self, args: &[&str]) -> assert_cmd::Command {
        let mut cmd = cargo_bin_cmd!("skill-curator");
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.current_dir(self.collection())
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.config_home())
            .env("XDG_STATE_HOME", self.state_home())
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .args(args);
        cmd
    }

    /// Records the local Skills' files in `lock.toml`.
    fn accept(&self) -> TestResult {
        self.command(&["materialize"]).assert().success();
        Ok(())
    }

    fn install(&self, args: &[&str]) -> Assert {
        let mut all = vec!["install"];
        all.extend_from_slice(args);
        self.command(&all).assert()
    }
}

fn stdout(assert: &Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn stderr(assert: &Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

/// Every entry below `root`, relative to it, sorted; fails on a symbolic
/// link.
fn entries(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let file_type = fs::symlink_metadata(&path)?.file_type();
            if file_type.is_symlink() {
                return Err(format!("{} is a symbolic link", path.display()).into());
            }
            found.push(
                path.strip_prefix(root)?
                    .to_str()
                    .ok_or("non-UTF-8 path")?
                    .to_owned(),
            );
            if file_type.is_dir() {
                pending.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn modified(path: &Path) -> Result<SystemTime, Box<dyn Error>> {
    Ok(fs::metadata(path)?.modified()?)
}

#[test]
fn installs_materialised_copies_into_every_enabled_destination() -> TestResult {
    let fixture = Fixture::new()?;
    let assert = fixture.install(&[]).success();
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "claude: added Skill `grill-me` at {}",
            fixture.claude_skills().join("grill-me").display()
        )),
        "{out}"
    );
    assert!(
        out.ends_with("6 added, 0 updated, 0 removed, 0 unchanged, 0 needing attention\n"),
        "{out}"
    );

    // Regular files only, and nothing but the Skills: no state, no staging.
    for skills in [fixture.claude_skills(), fixture.pi_skills()] {
        assert_eq!(
            entries(&skills)?,
            [
                "commit",
                "commit/SKILL.md",
                "grill-me",
                "grill-me/SKILL.md",
                "grill-me/references",
                "grill-me/references/guide.md",
            ]
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.claude_skills().join("commit/SKILL.md"))?,
        COMMIT
    );
    assert_eq!(
        fs::read_to_string(fixture.claude_skills().join("grill-me/references/guide.md"))?,
        "Guide.\n"
    );
    // The pi Destination's D1 rules apply to its copies only.
    assert_eq!(
        fs::read_to_string(fixture.pi_skills().join("commit/SKILL.md"))?,
        "---\nname: commit\ndescription: Write a commit.\n---\n# Commit\n"
    );
    // The disabled Codex Destination gets nothing.
    assert!(!fixture.home().join(".agents").exists());
    assert_eq!(entries(&fixture.home().join(".claude"))?.len(), 8);
    assert_eq!(entries(&fixture.home().join(".pi"))?.len(), 9);
    Ok(())
}

#[test]
fn repeated_installs_change_nothing() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    let skill_md = fixture.claude_skills().join("grill-me/SKILL.md");
    let before = (
        modified(&skill_md)?,
        modified(&fixture.claude_md())?,
        fs::read(fixture.state_file())?,
        entries(&fixture.home())?,
    );
    for args in [&[][..], &["--dry-run"][..]] {
        let assert = fixture.install(args).success();
        let out = stdout(&assert);
        assert!(
            out.starts_with("0 added, 0 updated, 0 removed, 6 unchanged, 0 needing attention"),
            "{out}"
        );
        assert_eq!(out.lines().count(), 1, "{out}");
    }
    let after = (
        modified(&skill_md)?,
        modified(&fixture.claude_md())?,
        fs::read(fixture.state_file())?,
        entries(&fixture.home())?,
    );
    assert_eq!(before, after);
    Ok(())
}

#[test]
fn machine_settings_enable_disable_and_relocate_destinations() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write_machine(
        "[destinations.claude]\nenabled = false\n\n[destinations.codex]\nenabled = true\n\n[destinations.pi]\nskills_dir = '~/pi-skills'\ninstruction_file = '~/pi/AGENTS.md'\n",
    )?;
    fixture.install(&[]).success();
    assert!(!fixture.home().join(".claude").exists());
    assert!(!fixture.home().join(".pi").exists());
    assert!(fixture.codex_skills().join("grill-me/SKILL.md").is_file());
    assert!(fixture.home().join("pi-skills/grill-me/SKILL.md").is_file());
    assert_eq!(
        fs::read_to_string(fixture.home().join("pi/AGENTS.md"))?,
        format!("{HEADER}\n\n{GIT_FRAGMENT}")
    );

    // `--machine-config` replaces the default location.
    let other = fixture.dir.path().join("other-machine.toml");
    fs::write(&other, "[destinations.pi]\nenabled = false\n")?;
    let other = other.to_str().ok_or("non-UTF-8 path")?;
    let assert = fixture
        .install(&["--machine-config", other, "--dry-run"])
        .success();
    let out = stdout(&assert);
    assert!(out.contains("claude: would add Skill `grill-me`"), "{out}");
    assert!(out.contains("pi: would remove Skill `grill-me`"), "{out}");
    // Codex is disabled again by the Manifest, so its copies are pruned.
    assert!(
        out.contains("codex: would remove Skill `grill-me`"),
        "{out}"
    );
    Ok(())
}

#[test]
fn codex_gets_its_metadata_file_and_long_description_warnings() -> TestResult {
    let fixture = Fixture::new()?;
    let long = "d".repeat(8_001);
    fixture.write_skill(
        "verbose",
        "SKILL.md",
        &format!("---\nname: verbose\ndescription: {long}\n---\nBody\n"),
    )?;
    fixture.write_manifest(&format!("{MANIFEST}\n[skills.verbose]\nmode = \"local\"\n"))?;
    fixture.accept()?;
    fixture.write_machine("[destinations.codex]\nenabled = true\n")?;
    let assert = fixture.install(&[]).success();
    let err = stderr(&assert);
    assert_eq!(
        err.lines()
            .filter(|line| line.starts_with("skill-curator: warning: "))
            .collect::<Vec<_>>(),
        [
            "skill-curator: warning: codex: Skill `verbose`: its description is 8001 characters long, more than the 8000 characters Codex lists by default for implicit invocation; shorten it with a Destination frontmatter rule or set `disable-model-invocation: true`"
        ],
        "{err}"
    );
    assert_eq!(
        fs::read_to_string(fixture.codex_skills().join("commit/agents/openai.yaml"))?,
        "policy:\n  allow_implicit_invocation: false\n"
    );
    assert!(!fixture.codex_skills().join("grill-me/agents").exists());
    assert!(!fixture.codex_skills().join("verbose/agents").exists());
    // Other harnesses get no metadata file.
    assert!(!fixture.claude_skills().join("commit/agents").exists());
    // The generated file is part of the installed copy: a repeat is a no-op.
    let assert = fixture.install(&[]).success();
    assert!(stdout(&assert).starts_with("0 added, 0 updated, 0 removed, 11 unchanged"));
    Ok(())
}

#[test]
fn fragments_assemble_in_declared_order_with_the_managed_header() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    assert_eq!(
        fs::read_to_string(fixture.claude_md())?,
        format!("{HEADER}\n\n{GIT_FRAGMENT}\n{CLAUDE_FRAGMENT}\n")
    );
    assert_eq!(
        fs::read_to_string(fixture.home().join(".pi/agent/AGENTS.md"))?,
        format!("{HEADER}\n\n{GIT_FRAGMENT}")
    );

    // A new order is a planned update in a dry run, written by install.
    fixture.write_manifest(&MANIFEST.replace(
        r#"["fragments/git.md", "fragments/claude.md"]"#,
        r#"["fragments/claude.md", "fragments/git.md"]"#,
    ))?;
    let assert = fixture.install(&["--dry-run"]).success();
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "claude: would update the instruction file {}",
            fixture.claude_md().display()
        )),
        "{out}"
    );
    assert!(out.contains("(dry run: nothing was written)"), "{out}");
    assert_eq!(
        fs::read_to_string(fixture.claude_md())?,
        format!("{HEADER}\n\n{GIT_FRAGMENT}\n{CLAUDE_FRAGMENT}\n")
    );
    fixture.install(&[]).success();
    assert_eq!(
        fs::read_to_string(fixture.claude_md())?,
        format!("{HEADER}\n\n{CLAUDE_FRAGMENT}\n\n{GIT_FRAGMENT}")
    );
    Ok(())
}

#[test]
fn manual_edits_of_an_instruction_file_block_overwrite_until_forced() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    let edited = format!("{HEADER}\n\nMy own rule.\n");
    fs::write(fixture.claude_md(), &edited)?;
    fs::write(
        fixture.collection().join("fragments/claude.md"),
        "Changed.\n",
    )?;

    for args in [&[][..], &["--dry-run"][..]] {
        let assert = fixture.install(args).code(FAILURE);
        let out = stdout(&assert);
        assert!(
            out.contains(&format!(
                "claude: the instruction file {} has manual edits (its contents differ from the copy skill-curator installed); it is not overwritten",
                fixture.claude_md().display()
            )),
            "{out}"
        );
        assert!(
            stderr(&assert).contains("1 path in a Destination needs attention"),
            "{}",
            stderr(&assert)
        );
        assert_eq!(fs::read_to_string(fixture.claude_md())?, edited);
    }

    let assert = fixture.install(&["--force"]).success();
    assert!(
        stdout(&assert).contains("overwriting its manual edits (--force)"),
        "{}",
        stdout(&assert)
    );
    assert_eq!(
        fs::read_to_string(fixture.claude_md())?,
        format!("{HEADER}\n\n{GIT_FRAGMENT}\nChanged.\n")
    );
    fixture.install(&[]).success();
    Ok(())
}

#[test]
fn an_unmanaged_instruction_file_is_refused_and_left_untouched() -> TestResult {
    let fixture = Fixture::new()?;
    fs::create_dir_all(fixture.home().join(".claude"))?;
    fs::write(fixture.claude_md(), "My hand-written rules.\n")?;
    for args in [&[][..], &["--force"][..]] {
        let assert = fixture.install(args).code(FAILURE);
        let out = stdout(&assert);
        assert!(
            out.contains(&format!(
                "claude: the instruction file {} is not managed by skill-curator (it already exists); refusing to overwrite it",
                fixture.claude_md().display()
            )),
            "{out}"
        );
        assert_eq!(
            fs::read_to_string(fixture.claude_md())?,
            "My hand-written rules.\n"
        );
    }
    // Everything else is installed.
    assert!(fixture.claude_skills().join("grill-me/SKILL.md").is_file());
    assert!(fixture.home().join(".pi/agent/AGENTS.md").is_file());
    Ok(())
}

#[test]
fn unmanaged_skills_are_preserved_and_collisions_refused() -> TestResult {
    let fixture = Fixture::new()?;
    let skills = fixture.claude_skills();
    // Installed by `npx skills` or by hand.
    fs::create_dir_all(skills.join("find-skills"))?;
    fs::write(skills.join("find-skills/SKILL.md"), "npx\n")?;
    fs::write(skills.join(".skill-lock.json"), "{}\n")?;
    fs::create_dir_all(skills.join("grill-me"))?;
    fs::write(skills.join("grill-me/SKILL.md"), "mine\n")?;
    symlink(fixture.dir.path(), skills.join("commit"))?;

    for args in [&[][..], &["--force"][..]] {
        let assert = fixture.install(args).code(FAILURE);
        let out = stdout(&assert);
        assert!(
            out.contains(&format!(
                "claude: Skill `grill-me` at {} is not managed by skill-curator (it already exists); refusing to overwrite it",
                skills.join("grill-me").display()
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "claude: Skill `commit` at {} is not managed by skill-curator (",
                skills.join("commit").display()
            )),
            "{out}"
        );
        assert!(
            stderr(&assert).contains("2 paths in Destinations need attention"),
            "{}",
            stderr(&assert)
        );
    }
    assert_eq!(
        fs::read_to_string(skills.join("grill-me/SKILL.md"))?,
        "mine\n"
    );
    assert_eq!(
        fs::read_to_string(skills.join("find-skills/SKILL.md"))?,
        "npx\n"
    );
    assert_eq!(fs::read_to_string(skills.join(".skill-lock.json"))?, "{}\n");
    assert!(
        fs::symlink_metadata(skills.join("commit"))?
            .file_type()
            .is_symlink()
    );
    // Other Destinations are installed.
    assert!(fixture.pi_skills().join("grill-me/SKILL.md").is_file());

    // Once the colliding Skill is moved away, install takes over.
    fs::remove_dir_all(skills.join("grill-me"))?;
    fs::remove_file(skills.join("commit"))?;
    fixture.install(&[]).success();
    assert_eq!(
        fs::read_to_string(skills.join("grill-me/SKILL.md"))?,
        GRILL_ME
    );
    assert_eq!(
        fs::read_to_string(skills.join("find-skills/SKILL.md"))?,
        "npx\n"
    );
    assert_eq!(fs::read_to_string(skills.join(".skill-lock.json"))?, "{}\n");
    Ok(())
}

#[test]
fn skills_removed_from_the_manifest_are_pruned() -> TestResult {
    let fixture = Fixture::new()?;
    fs::create_dir_all(fixture.claude_skills().join("find-skills"))?;
    fs::write(
        fixture.claude_skills().join("find-skills/SKILL.md"),
        "npx\n",
    )?;
    fixture.install(&[]).success();
    fixture.write_manifest(&MANIFEST.replace("[skills.commit]\nmode = \"local\"\n", ""))?;

    let assert = fixture.install(&["--dry-run"]).success();
    assert!(
        stdout(&assert).contains("claude: would remove Skill `commit`"),
        "{}",
        stdout(&assert)
    );
    assert!(fixture.claude_skills().join("commit").is_dir());

    let assert = fixture.install(&[]).success();
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "claude: removed Skill `commit` at {}, which is no longer declared there",
            fixture.claude_skills().join("commit").display()
        )),
        "{out}"
    );
    assert!(out.contains("pi: removed Skill `commit`"), "{out}");
    assert_eq!(
        entries(&fixture.claude_skills())?,
        [
            "find-skills",
            "find-skills/SKILL.md",
            "grill-me",
            "grill-me/SKILL.md",
            "grill-me/references",
            "grill-me/references/guide.md",
        ]
    );
    let state = fs::read_to_string(fixture.state_file())?;
    assert!(!state.contains("commit"), "{state}");
    fixture.install(&[]).success();
    Ok(())
}

#[test]
fn manual_edits_in_a_pruned_skill_block_deletion_until_forced() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    let edited = fixture.claude_skills().join("commit/notes.md");
    fs::write(&edited, "My notes.\n")?;
    fixture.write_manifest(&MANIFEST.replace("[skills.commit]\nmode = \"local\"\n", ""))?;

    let assert = fixture.install(&[]).code(FAILURE);
    assert!(
        stdout(&assert).contains("claude: Skill `commit` at "),
        "{}",
        stdout(&assert)
    );
    assert!(
        stdout(&assert).contains("has manual edits (its files differ from the copy skill-curator installed); it is not deleted"),
        "{}",
        stdout(&assert)
    );
    assert!(edited.is_file());
    assert!(!fixture.pi_skills().join("commit").exists());

    let assert = fixture.install(&["--force"]).success();
    assert!(stdout(&assert).contains("deleting its manual edits (--force)"));
    assert!(!fixture.claude_skills().join("commit").exists());
    Ok(())
}

#[test]
fn manual_edits_in_an_installed_skill_block_overwrite_until_forced() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    let installed = fixture.claude_skills().join("grill-me/SKILL.md");
    fs::write(&installed, "edited in the Destination\n")?;

    // Drift alone is reported, even without a change in the Collection.
    let assert = fixture.install(&[]).code(FAILURE);
    assert!(
        stdout(&assert).contains("claude: Skill `grill-me` at "),
        "{}",
        stdout(&assert)
    );
    fixture.write_skill("grill-me", "SKILL.md", &GRILL_ME.replace("one", "a single"))?;
    fixture.accept()?;
    for (args, pi) in [
        (&["--dry-run"][..], "pi: would update Skill `grill-me`"),
        (&[][..], "pi: updated Skill `grill-me`"),
    ] {
        let assert = fixture.install(args).code(FAILURE);
        let out = stdout(&assert);
        assert!(
            out.contains("has manual edits (its files differ from the copy skill-curator installed); it is not overwritten; keep the edits in the Collection instead, or run `skill-curator install --force` to overwrite them"),
            "{out}"
        );
        // The pi copy has no edits, so it is updated.
        assert!(out.contains(pi), "{out}");
        assert_eq!(
            fs::read_to_string(&installed)?,
            "edited in the Destination\n"
        );
    }
    assert!(
        fs::read_to_string(fixture.pi_skills().join("grill-me/SKILL.md"))?.contains("a single")
    );

    fixture.install(&["--force"]).success();
    assert!(fs::read_to_string(&installed)?.contains("Ask a single question"));
    let assert = fixture.install(&[]).success();
    assert!(stdout(&assert).starts_with("0 added, 0 updated, 0 removed, 6 unchanged"));
    Ok(())
}

#[test]
fn a_symlink_replacing_a_managed_skill_is_drift_and_never_followed() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    let outside = fixture.dir.path().join("outside");
    fs::create_dir(&outside)?;
    fs::write(outside.join("SKILL.md"), "outside\n")?;
    let link_path = fixture.claude_skills().join("grill-me");
    fs::remove_dir_all(&link_path)?;
    symlink(&outside, &link_path)?;

    let assert = fixture.install(&[]).code(FAILURE);
    assert!(
        stdout(&assert).contains("is a symbolic link"),
        "{}",
        stdout(&assert)
    );
    fixture.install(&["--force"]).success();
    assert!(fs::symlink_metadata(&link_path)?.is_dir());
    assert_eq!(fs::read_to_string(outside.join("SKILL.md"))?, "outside\n");
    assert_eq!(entries(&outside)?, ["SKILL.md"]);
    Ok(())
}

#[test]
fn disabling_a_destination_prunes_what_install_put_there() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    fs::write(fixture.home().join(".pi/agent/settings.json"), "{}\n")?;
    fixture.write_machine("[destinations.pi]\nenabled = false\n")?;
    let assert = fixture.install(&[]).success();
    let out = stdout(&assert);
    assert!(out.contains("pi: removed Skill `grill-me`"), "{out}");
    assert!(out.contains("pi: removed the instruction file"), "{out}");
    assert_eq!(
        entries(&fixture.home().join(".pi"))?,
        ["agent", "agent/settings.json", "agent/skills"]
    );
    assert!(fixture.claude_skills().join("grill-me").is_dir());
    let state = fs::read_to_string(fixture.state_file())?;
    assert!(!state.contains(".pi"), "{state}");
    Ok(())
}

#[test]
fn a_dry_run_writes_nothing() -> TestResult {
    let fixture = Fixture::new()?;
    let assert = fixture.install(&["--dry-run"]).success();
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "pi: would add the instruction file {}",
            fixture.home().join(".pi/agent/AGENTS.md").display()
        )),
        "{out}"
    );
    assert!(
        out.ends_with("6 added, 0 updated, 0 removed, 0 unchanged, 0 needing attention (dry run: nothing was written)\n"),
        "{out}"
    );
    assert_eq!(entries(&fixture.home())?, Vec::<String>::new());
    assert!(!fixture.state_home().exists());
    Ok(())
}

#[test]
fn ownership_state_lives_in_the_state_directory_only() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    assert_eq!(
        entries(&fixture.state_home())?,
        ["skill-curator", "skill-curator/install.toml"]
    );
    let state = fs::read_to_string(fixture.state_file())?;
    assert!(
        state.contains(&format!(
            "[skills_dirs.\"{}\"]",
            fixture.claude_skills().display()
        )),
        "{state}"
    );
    assert!(
        state.contains(&format!(
            "[instruction_files.\"{}\"]",
            fixture.claude_md().display()
        )),
        "{state}"
    );
    // No hidden or state file lands in a Destination.
    for path in entries(&fixture.home())? {
        assert!(
            !path
                .split('/')
                .any(|part| part.starts_with('.') && part != ".claude" && part != ".pi"),
            "{path}"
        );
        assert!(!path.ends_with(".toml"), "{path}");
    }

    // Without `XDG_STATE_HOME`, the state goes to `~/.local/state`.
    let other = Fixture::new()?;
    other
        .command(&["install"])
        .env_remove("XDG_STATE_HOME")
        .assert()
        .success();
    assert!(
        other
            .home()
            .join(".local/state/skill-curator/install.toml")
            .is_file()
    );
    assert!(!other.state_home().exists());
    Ok(())
}

#[test]
fn skills_that_are_not_accepted_are_not_installed() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.install(&[]).success();
    fixture.write_skill("grill-me", "SKILL.md", "edited, not accepted\n")?;
    let assert = fixture.install(&[]).code(FAILURE);
    let out = stdout(&assert);
    assert!(
        out.contains("Skill `grill-me` is not installed: `skills/grill-me/` differs from the accepted output recorded in lock.toml; run `skill-curator materialize` to accept or reset the edits"),
        "{out}"
    );
    // The installed copies are kept, not pruned.
    assert_eq!(
        fs::read_to_string(fixture.claude_skills().join("grill-me/SKILL.md"))?,
        GRILL_ME
    );

    // A declared Skill never accepted is reported too.
    fixture.write_manifest(&format!("{MANIFEST}\n[skills.new]\nmode = \"local\"\n"))?;
    fixture.write_skill("new", "SKILL.md", "---\nname: new\n---\n")?;
    let assert = fixture.install(&[]).code(FAILURE);
    assert!(
        stdout(&assert).contains("Skill `new` is not installed: it is not materialised yet"),
        "{}",
        stdout(&assert)
    );
    assert!(!fixture.claude_skills().join("new").exists());
    Ok(())
}

#[test]
fn a_symlinked_skills_directory_is_refused() -> TestResult {
    let fixture = Fixture::new()?;
    let elsewhere = fixture.dir.path().join("elsewhere");
    fs::create_dir(&elsewhere)?;
    fs::create_dir_all(fixture.home().join(".claude"))?;
    symlink(&elsewhere, fixture.claude_skills())?;
    let assert = fixture.install(&[]).code(FAILURE);
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "claude: the Skills directory {}: failed: it is a symbolic link",
            fixture.claude_skills().display()
        )),
        "{out}"
    );
    assert_eq!(entries(&elsewhere)?, Vec::<String>::new());
    assert!(fixture.pi_skills().join("grill-me").is_dir());
    Ok(())
}

#[test]
fn the_state_file_may_not_live_inside_a_destination() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .command(&["install"])
        .env("XDG_STATE_HOME", fixture.claude_skills())
        .assert()
        .code(FAILURE)
        .stderr(predicate::str::contains(
            "would be written inside Destination `claude`",
        ));
    assert!(!fixture.home().join(".claude").exists());
    Ok(())
}
