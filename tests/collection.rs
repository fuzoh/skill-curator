//! Loading a Collection from disk: Manifest and lock parsing, validation
//! against the files the Manifest references, machine-local Destination
//! settings, and input and output hashing.

use std::error::Error;
use std::fs;
use std::path::Path;

use skill_curator::collection::lock::{LOCK_FORMAT, Lock, LockedSkill};
use skill_curator::collection::{Collection, CollectionError};
use skill_curator::safety::Name;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn Error>>;

const MANIFEST: &str = r#"
[sources.mattpocock]
url = "https://github.com/mattpocock/skills.git"

[sources.pstack]
url = "https://github.com/cursor/plugins.git"
branch = "main"

[presets.cursor-cleanup]
frontmatter = { remove = ["icon", "color"] }
files = { exclude = ["automations/**"] }

[skills.grill-me]
mode = "copy"
source = "mattpocock"
upstream_path = "skills/grill-me"

[skills.grilling]
mode = "adapt"
source = "mattpocock"
upstream_path = "skills/grilling"
presets = ["cursor-cleanup"]
intent = "intents/grilling.md"
frontmatter = { set = { disable-model-invocation = true } }

[skills.code-review]
mode = "composition"
intent = "intents/code-review.md"

[skills.code-review.inputs.review]
source = "mattpocock"
upstream_path = "skills/review"

[skills.code-review.inputs.critique]
source = "pstack"
upstream_path = "pstack/critique"

[skills.fr-learner-voice]
mode = "local"

[destinations.claude]
harness = "claude-code"
instructions = { file = "~/.claude/CLAUDE.md", fragments = ["fragments/git.md", "fragments/claude.md"] }

[destinations.pi]
harness = "pi"
instructions = { file = "~/.pi/agent/AGENTS.md", fragments = ["fragments/git.md"] }

[destinations.codex]
harness = "codex"
enabled = false
"#;

/// Writes `contents` to `root/relative`, creating parent directories.
fn write(root: &Path, relative: &str, contents: &str) -> TestResult {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

/// A complete, valid Collection in a temporary directory.
fn collection() -> Result<TempDir, Box<dyn Error>> {
    let dir = TempDir::new()?;
    let root = dir.path();
    write(root, "manifest.toml", MANIFEST)?;
    write(root, "intents/grilling.md", "Keep turns short.\n")?;
    write(root, "intents/code-review.md", "Combine both reviews.\n")?;
    write(root, "fragments/git.md", "Use Conventional Commits.\n")?;
    write(root, "fragments/claude.md", "Use ctx7 for docs.\n")?;
    write(
        root,
        "skills/fr-learner-voice/SKILL.md",
        "---\nname: fr-learner-voice\n---\nBody\n",
    )?;
    Ok(dir)
}

fn name(raw: &str) -> Result<Name, Box<dyn Error>> {
    Ok(Name::new(raw)?)
}

fn load_problems(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    match Collection::load(root) {
        Err(CollectionError::Invalid { problems, .. }) => {
            Ok(problems.iter().map(ToString::to_string).collect())
        }
        Err(other) => Err(format!("expected validation problems, got: {other}").into()),
        Ok(_) => Err("expected validation problems, got a loaded Collection".into()),
    }
}

#[test]
fn loads_a_valid_collection_without_lock() -> TestResult {
    let dir = collection()?;
    let collection = Collection::load(dir.path())?;
    assert_eq!(collection.root(), dir.path());
    assert_eq!(collection.manifest().skills.len(), 4);
    assert_eq!(collection.lock(), &Lock::default());
    assert_eq!(
        collection.skill_dir(&name("grilling")?),
        dir.path().join("skills/grilling")
    );
    Ok(())
}

#[test]
fn loads_the_lock_when_present() -> TestResult {
    let dir = collection()?;
    let digest = Collection::load(dir.path())?.output_digest(&name("fr-learner-voice")?)?;
    let mut lock = Lock::default();
    lock.skills.insert(
        name("fr-learner-voice")?,
        LockedSkill {
            output_hash: digest,
            inputs_hash: None,
            provenance: Vec::new(),
        },
    );
    write(dir.path(), "lock.toml", &lock.to_toml()?)?;
    assert_eq!(Collection::load(dir.path())?.lock(), &lock);
    Ok(())
}

#[test]
fn reports_a_missing_manifest() -> TestResult {
    let dir = TempDir::new()?;
    let err = Collection::load(dir.path())
        .err()
        .ok_or("loading an empty directory must fail")?;
    assert!(matches!(err, CollectionError::Read { .. }), "{err}");
    assert!(err.to_string().contains("manifest.toml"), "{err}");
    Ok(())
}

#[test]
fn reports_parse_errors_with_file_and_line() -> TestResult {
    let dir = collection()?;
    write(
        dir.path(),
        "manifest.toml",
        "[destinations.d]\nharness = 'cursor'\n",
    )?;
    let err = Collection::load(dir.path())
        .err()
        .ok_or("an unknown harness must fail")?
        .to_string();
    assert!(err.contains("manifest.toml"), "{err}");
    assert!(err.contains("line 2"), "{err}");
    assert!(err.contains("unknown variant `cursor`"), "{err}");
    Ok(())
}

#[test]
fn reports_every_missing_referenced_file() -> TestResult {
    let dir = collection()?;
    fs::remove_file(dir.path().join("intents/code-review.md"))?;
    fs::remove_file(dir.path().join("fragments/claude.md"))?;
    fs::remove_dir_all(dir.path().join("skills/fr-learner-voice"))?;
    let problems = load_problems(dir.path())?;
    assert_eq!(
        problems,
        [
            "skills.code-review.intent: written intent `intents/code-review.md` does not exist; create it in the Collection",
            "skills.fr-learner-voice: local Skill has no `skills/fr-learner-voice/SKILL.md`; author it in the Collection",
            "destinations.claude.instructions.fragments: Fragment `fragments/claude.md` does not exist; create it in the Collection",
        ]
    );
    Ok(())
}

#[test]
fn reports_reference_problems_together_with_missing_files() -> TestResult {
    let dir = collection()?;
    let manifest = MANIFEST
        .replace("presets = [\"cursor-cleanup\"]", "presets = [\"cursor\"]")
        .replace("intent = \"intents/code-review.md\"\n", "")
        .replace("source = \"pstack\"", "source = \"cursor\"");
    write(dir.path(), "manifest.toml", &manifest)?;
    fs::remove_file(dir.path().join("intents/grilling.md"))?;
    let problems = load_problems(dir.path())?;
    assert_eq!(problems.len(), 4, "{problems:#?}");
    assert!(
        problems[0].starts_with(
            "skills.code-review.inputs.critique.source: Source `cursor` is not declared"
        )
    );
    assert!(problems[1].starts_with("skills.code-review: a Composition requires a written intent"));
    assert!(problems[2].starts_with("skills.grilling.presets: preset `cursor` is not declared"));
    assert!(problems[3].starts_with(
        "skills.grilling.intent: written intent `intents/grilling.md` does not exist"
    ));

    let message = Collection::load(dir.path())
        .err()
        .ok_or("must fail")?
        .to_string();
    assert!(message.starts_with("invalid "), "{message}");
    assert!(
        message.contains("manifest.toml:\n  - skills.code-review.inputs.critique.source"),
        "{message}"
    );
    Ok(())
}

#[test]
fn rejects_an_unsupported_lock_format() -> TestResult {
    let dir = collection()?;
    write(dir.path(), "lock.toml", "format = 2\n")?;
    let problems = load_problems(dir.path())?;
    assert_eq!(
        problems,
        [format!(
            "format: lock format 2 is not supported by this release, which reads format {LOCK_FORMAT}; upgrade skill-curator"
        )]
    );
    Ok(())
}

#[test]
fn missing_machine_config_keeps_portable_destinations() -> TestResult {
    let dir = collection()?;
    let home = dir.path().join("home");
    let collection = Collection::load(dir.path())?;
    let destinations = collection.destinations(&dir.path().join("absent/machine.toml"), &home)?;
    let summary = destinations
        .iter()
        .map(|destination| (destination.name.as_str(), destination.enabled))
        .collect::<Vec<_>>();
    assert_eq!(summary, [("claude", true), ("codex", false), ("pi", true)]);
    assert_eq!(destinations[0].skills_dir, home.join(".claude/skills"));
    assert_eq!(
        destinations[0]
            .instructions
            .as_ref()
            .map(|i| i.file.clone()),
        Some(home.join(".claude/CLAUDE.md"))
    );
    Ok(())
}

#[test]
fn machine_config_merges_over_portable_destinations() -> TestResult {
    let dir = collection()?;
    let home = dir.path().join("home");
    let machine = dir.path().join("config/machine.toml");
    write(
        dir.path(),
        "config/machine.toml",
        "[destinations.claude]\nenabled = false\n\n[destinations.codex]\nenabled = true\nskills_dir = '/opt/codex/skills'\n",
    )?;
    let collection = Collection::load(dir.path())?;
    let destinations = collection.destinations(&machine, &home)?;
    let summary = destinations
        .iter()
        .map(|destination| {
            (
                destination.name.as_str(),
                destination.enabled,
                destination.skills_dir.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            ("claude", false, home.join(".claude/skills")),
            ("codex", true, Path::new("/opt/codex/skills").to_path_buf()),
            ("pi", true, home.join(".pi/agent/skills")),
        ]
    );
    Ok(())
}

#[test]
fn machine_config_errors_name_the_machine_file() -> TestResult {
    let dir = collection()?;
    let machine = dir.path().join("machine.toml");
    let collection = Collection::load(dir.path())?;

    write(
        dir.path(),
        "machine.toml",
        "[destinations.cursor]\nenabled = true\n",
    )?;
    let err = collection
        .destinations(&machine, dir.path())
        .err()
        .ok_or("an undeclared Destination must fail")?
        .to_string();
    assert!(
        err.contains("machine.toml:\n  - destinations.cursor"),
        "{err}"
    );

    write(
        dir.path(),
        "machine.toml",
        "[destinations.pi]\nskills_dir = 'pi'\n",
    )?;
    let err = collection
        .destinations(&machine, dir.path())
        .err()
        .ok_or("a relative path must fail")?
        .to_string();
    assert!(err.contains("machine.toml"), "{err}");
    assert!(err.contains("not a valid installation path"), "{err}");
    Ok(())
}

/// Inputs hash of every non-local Skill of the Collection at `root`.
fn inputs_digests(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let collection = Collection::load(root)?;
    let mut digests = Vec::new();
    for skill in ["grill-me", "grilling", "code-review"] {
        let digest = collection
            .inputs_digest(&name(skill)?)?
            .ok_or("upstream Skills have inputs")?;
        digests.push(digest.as_str().to_owned());
    }
    Ok(digests)
}

#[test]
fn inputs_digests_are_deterministic_and_independent_of_formatting() -> TestResult {
    let dir = collection()?;
    let original = inputs_digests(dir.path())?;
    assert_eq!(inputs_digests(dir.path())?, original);
    let unique = original.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        original.len(),
        "Skills must not share an inputs hash"
    );

    // Same declarations, reordered and reformatted.
    let reordered = MANIFEST
        .replace(
            "frontmatter = { remove = [\"icon\", \"color\"] }\nfiles = { exclude = [\"automations/**\"] }",
            "files.exclude = [ 'automations/**' ]\n\nfrontmatter.remove = [ 'icon', 'color' ]",
        )
        .replace(
            "presets = [\"cursor-cleanup\"]\nintent = \"intents/grilling.md\"",
            "intent = 'intents/grilling.md'\npresets = ['cursor-cleanup']",
        );
    assert_ne!(reordered, MANIFEST);
    write(dir.path(), "manifest.toml", &reordered)?;
    assert_eq!(inputs_digests(dir.path())?, original);

    let local = Collection::load(dir.path())?.inputs_digest(&name("fr-learner-voice")?)?;
    assert_eq!(local, None);
    Ok(())
}

#[test]
fn inputs_digests_detect_rule_preset_and_intent_changes() -> TestResult {
    let dir = collection()?;
    let original = inputs_digests(dir.path())?;

    let changes = [
        // A preset referenced by `grilling`.
        ("\"automations/**\"", "\"automations/*\""),
        // A D1 rule of `grilling`.
        (
            "disable-model-invocation = true",
            "disable-model-invocation = false",
        ),
        // A Composition input path.
        ("pstack/critique", "pstack/critiques"),
    ];
    let expected_changed = [1, 1, 2];
    for ((from, to), changed_index) in changes.into_iter().zip(expected_changed) {
        write(dir.path(), "manifest.toml", &MANIFEST.replace(from, to))?;
        let digests = inputs_digests(dir.path())?;
        for (index, (now, before)) in digests.iter().zip(&original).enumerate() {
            assert_eq!(
                now != before,
                index == changed_index,
                "{from} -> {to}, Skill {index}"
            );
        }
    }

    write(dir.path(), "manifest.toml", MANIFEST)?;
    write(
        dir.path(),
        "intents/code-review.md",
        "Combine both reviews!\n",
    )?;
    let digests = inputs_digests(dir.path())?;
    assert_eq!(digests[..2], original[..2]);
    assert_ne!(digests[2], original[2]);
    Ok(())
}

#[test]
fn inputs_digest_rejects_undeclared_skills() -> TestResult {
    let dir = collection()?;
    let collection = Collection::load(dir.path())?;
    let err = collection
        .inputs_digest(&name("unknown")?)
        .err()
        .ok_or("an undeclared Skill must fail")?;
    assert_eq!(
        err.to_string(),
        "Skill `unknown` is not declared in manifest.toml"
    );
    Ok(())
}

#[test]
fn output_digest_tracks_the_materialised_skill() -> TestResult {
    let dir = collection()?;
    let collection = Collection::load(dir.path())?;
    let skill = name("fr-learner-voice")?;
    let before = collection.output_digest(&skill)?;
    write(
        dir.path(),
        "skills/fr-learner-voice/SKILL.md",
        "---\nname: fr-learner-voice\n---\nBody.\n",
    )?;
    assert_ne!(collection.output_digest(&skill)?, before);
    Ok(())
}
