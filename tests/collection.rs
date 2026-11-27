//! Loading a Collection from disk: Manifest and lock parsing, validation
//! against the files the Manifest references, machine-local Destination
//! settings, and input and output hashing.

use std::error::Error;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use skill_curator::collection::lock::{LOCK_FORMAT, Lock, LockedSkill};
use skill_curator::collection::{Collection, CollectionError};
use skill_curator::hash::HashError;
use skill_curator::safety::{HomeDir, Name};
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
    populate(dir.path())?;
    Ok(dir)
}

/// Writes a complete, valid Collection at `root`.
fn populate(root: &Path) -> TestResult {
    write(root, "manifest.toml", MANIFEST)?;
    write(root, "intents/grilling.md", "Keep turns short.\n")?;
    write(root, "intents/code-review.md", "Combine both reviews.\n")?;
    write(root, "fragments/git.md", "Use Conventional Commits.\n")?;
    write(root, "fragments/claude.md", "Use ctx7 for docs.\n")?;
    write(
        root,
        "skills/fr-learner-voice/SKILL.md",
        "---\nname: fr-learner-voice\n---\nBody\n",
    )
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
fn reports_a_newer_lock_format_before_its_schema() -> TestResult {
    let dir = collection()?;
    for lock in [
        "format = 2\nfoo = 1\n",
        "format = 2\n[skills.grill-me]\noutput_hash = 'blake3:abc'\n",
    ] {
        write(dir.path(), "lock.toml", lock)?;
        let problems = load_problems(dir.path())?;
        assert_eq!(
            problems,
            [format!(
                "format: lock format 2 is not supported by this release, which reads format {LOCK_FORMAT}; upgrade skill-curator"
            )],
            "{lock}"
        );
    }

    // A current lock that breaks the schema still gets the precise error.
    write(dir.path(), "lock.toml", "format = 1\nfoo = 1\n")?;
    let err = Collection::load(dir.path())
        .err()
        .ok_or("an unknown lock key must fail")?;
    assert!(matches!(err, CollectionError::Parse { .. }), "{err}");
    assert!(err.to_string().contains("unknown field `foo`"), "{err}");
    Ok(())
}

#[test]
fn reports_lock_consistency_problems() -> TestResult {
    let dir = collection()?;
    let revision = "0".repeat(40);
    let hash = format!("sha256:{}", "a".repeat(64));
    let entry = format!(
        "[[skills.code-review.provenance]]\ninput = 'review'\nsource = 'mattpocock'\nurl = 'u'\nupstream_path = 'skills/review'\nrevision = '{revision}'\n"
    );
    write(
        dir.path(),
        "lock.toml",
        &format!("format = 1\n[skills.code-review]\noutput_hash = '{hash}'\n{entry}{entry}"),
    )?;
    let problems = load_problems(dir.path())?;
    assert_eq!(
        problems,
        [
            "skills.code-review.provenance: input `review` has more than one provenance entry; record each Composition input once"
        ]
    );
    Ok(())
}

#[test]
fn rejects_referenced_files_reached_through_symbolic_links() -> TestResult {
    let outside = TempDir::new()?;
    write(outside.path(), "grilling.md", "Outside intent.\n")?;
    write(outside.path(), "code-review.md", "Outside intent.\n")?;
    write(outside.path(), "claude.md", "Outside Fragment.\n")?;
    write(outside.path(), "skill/SKILL.md", "---\nname: x\n---\n")?;

    let dir = collection()?;
    let root = dir.path();
    // A symlinked directory, a symlinked file, and a symlinked Skill.
    fs::remove_dir_all(root.join("intents"))?;
    symlink(outside.path(), root.join("intents"))?;
    fs::remove_file(root.join("fragments/claude.md"))?;
    symlink(
        outside.path().join("claude.md"),
        root.join("fragments/claude.md"),
    )?;
    fs::remove_dir_all(root.join("skills/fr-learner-voice"))?;
    symlink(
        outside.path().join("skill"),
        root.join("skills/fr-learner-voice"),
    )?;

    let problems = load_problems(root)?;
    let intents = root.join("intents");
    let skill = root.join("skills/fr-learner-voice");
    let fragment = root.join("fragments/claude.md");
    assert_eq!(
        problems,
        [
            format!(
                "skills.code-review.intent: written intent `intents/code-review.md` is reached through the symbolic link {}; replace it with a regular file in the Collection",
                intents.display()
            ),
            format!(
                "skills.fr-learner-voice: local Skill `skills/fr-learner-voice/SKILL.md` is reached through the symbolic link {}; keep the Skill as regular files in the Collection",
                skill.display()
            ),
            format!(
                "skills.grilling.intent: written intent `intents/grilling.md` is reached through the symbolic link {}; replace it with a regular file in the Collection",
                intents.display()
            ),
            format!(
                "destinations.claude.instructions.fragments: Fragment `fragments/claude.md` is reached through the symbolic link {}; replace it with a regular file in the Collection",
                fragment.display()
            ),
        ]
    );
    Ok(())
}

#[test]
fn rejects_a_referenced_path_that_is_not_a_regular_file() -> TestResult {
    let dir = collection()?;
    fs::remove_file(dir.path().join("fragments/git.md"))?;
    fs::create_dir(dir.path().join("fragments/git.md"))?;
    let problems = load_problems(dir.path())?;
    assert_eq!(
        problems,
        [
            "destinations.claude.instructions.fragments: Fragment `fragments/git.md` is not a regular file",
            "destinations.pi.instructions.fragments: Fragment `fragments/git.md` is not a regular file",
        ]
    );
    Ok(())
}

#[test]
fn inputs_digest_refuses_an_intent_replaced_by_a_symbolic_link() -> TestResult {
    let outside = TempDir::new()?;
    write(outside.path(), "secret", "not an intent\n")?;
    let dir = collection()?;
    let collection = Collection::load(dir.path())?;
    fs::remove_file(dir.path().join("intents/grilling.md"))?;
    symlink(
        outside.path().join("secret"),
        dir.path().join("intents/grilling.md"),
    )?;
    let err = collection
        .inputs_digest(&name("grilling")?)
        .err()
        .ok_or("a symlinked intent must not be hashed")?;
    assert!(
        err.to_string()
            .contains("skills.grilling.intent: written intent `intents/grilling.md` is reached through the symbolic link"),
        "{err}"
    );
    Ok(())
}

#[test]
fn output_digest_refuses_a_skill_reached_through_a_symbolic_link() -> TestResult {
    let outside = TempDir::new()?;
    write(
        outside.path(),
        "grilling/SKILL.md",
        "---\nname: grilling\n---\n",
    )?;
    let dir = collection()?;
    let collection = Collection::load(dir.path())?;

    // The Skill directory itself is a link.
    let link = dir.path().join("skills/grilling");
    symlink(outside.path().join("grilling"), &link)?;
    let err = collection
        .output_digest(&name("grilling")?)
        .err()
        .ok_or("a symlinked Skill directory must not be hashed")?;
    assert!(
        matches!(err, CollectionError::Hash(HashError::Symlink { ref path }) if *path == link),
        "{err}"
    );

    // The whole `skills/` directory is a link.
    fs::rename(dir.path().join("skills"), outside.path().join("skills"))?;
    symlink(outside.path().join("skills"), dir.path().join("skills"))?;
    let err = collection
        .output_digest(&name("fr-learner-voice")?)
        .err()
        .ok_or("a Skill under a symlinked `skills/` must not be hashed")?;
    assert!(
        matches!(err, CollectionError::Hash(HashError::Symlink { ref path }) if *path == dir.path().join("skills")),
        "{err}"
    );
    Ok(())
}

#[test]
fn missing_machine_config_keeps_portable_destinations() -> TestResult {
    let dir = collection()?;
    let home_dir = TempDir::new()?;
    let home = HomeDir::new(home_dir.path())?;
    let collection = Collection::load(dir.path())?;
    let destinations = collection.destinations(&dir.path().join("absent/machine.toml"), &home)?;
    let home = home.as_path();
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
    let home_dir = TempDir::new()?;
    let home = HomeDir::new(home_dir.path())?;
    let machine = dir.path().join("config/machine.toml");
    write(
        dir.path(),
        "config/machine.toml",
        "[destinations.claude]\nenabled = false\n\n[destinations.codex]\nenabled = true\nskills_dir = '/opt/codex/skills'\n",
    )?;
    let collection = Collection::load(dir.path())?;
    let destinations = collection.destinations(&machine, &home)?;
    let home = home.as_path();
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
    let home_dir = TempDir::new()?;
    let home = HomeDir::new(home_dir.path())?;
    let collection = Collection::load(dir.path())?;

    write(
        dir.path(),
        "machine.toml",
        "[destinations.cursor]\nenabled = true\n",
    )?;
    let err = collection
        .destinations(&machine, &home)
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
        .destinations(&machine, &home)
        .err()
        .ok_or("a relative path must fail")?
        .to_string();
    assert!(err.contains("machine.toml"), "{err}");
    assert!(err.contains("not a valid installation path"), "{err}");
    Ok(())
}

#[test]
fn destinations_may_not_overlap_the_collection() -> TestResult {
    let home_dir = TempDir::new()?;
    let home_path = fs::canonicalize(home_dir.path())?;
    let home = HomeDir::new(&home_path)?;
    let root = home_path.join("lab/collection");
    populate(&root)?;
    // The same Collection, also reached through a symbolic link.
    let link = home_path.join("link");
    symlink(&root, &link)?;
    let machine = home_path.join("machine.toml");

    for (loaded_from, settings, expected) in [
        (
            &root,
            "[destinations.pi]\nskills_dir = '~/lab/collection/skills'\n",
            format!(
                "destinations.pi: its Skills directory {} is inside the Collection root {}; choose a path outside the Collection",
                root.join("skills").display(),
                root.display()
            ),
        ),
        (
            &root,
            "[destinations.claude]\ninstruction_file = '~/lab/collection/manifest.toml'\n",
            format!(
                "destinations.claude: its instruction file {} is inside the Collection root {}; choose a path outside the Collection",
                root.join("manifest.toml").display(),
                root.display()
            ),
        ),
        (
            &root,
            "[destinations.pi]\nskills_dir = '~/lab'\n",
            format!(
                "destinations.pi: its Skills directory {} contains the Collection root {}; choose a path outside the Collection",
                home_path.join("lab").display(),
                root.display()
            ),
        ),
        // Loaded through the link: both the link and the real path count.
        (
            &link,
            "[destinations.pi]\nskills_dir = '~/link/skills'\n",
            format!(
                "destinations.pi: its Skills directory {} is inside the Collection root {}; choose a path outside the Collection",
                link.join("skills").display(),
                link.display()
            ),
        ),
        (
            &link,
            "[destinations.pi]\nskills_dir = '~/lab/collection/skills'\n",
            format!(
                "destinations.pi: its Skills directory {} is inside the Collection root {}; choose a path outside the Collection",
                root.join("skills").display(),
                root.display()
            ),
        ),
    ] {
        fs::write(&machine, settings)?;
        let collection = Collection::load(loaded_from)?;
        match collection.destinations(&machine, &home) {
            Err(CollectionError::Invalid { path, problems }) => {
                assert_eq!(path, machine);
                let problems = problems.iter().map(ToString::to_string).collect::<Vec<_>>();
                assert_eq!(problems, [expected], "{settings}");
            }
            other => return Err(format!("{settings}: expected a problem, got {other:?}").into()),
        }
    }

    // A sibling of the Collection is outside it.
    fs::write(
        &machine,
        "[destinations.pi]\nskills_dir = '~/lab/collection-skills'\n",
    )?;
    Collection::load(&root)?.destinations(&machine, &home)?;
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
fn inputs_digests_ignore_preset_names() -> TestResult {
    let dir = collection()?;
    let original = inputs_digests(dir.path())?;
    write(
        dir.path(),
        "manifest.toml",
        &MANIFEST.replace("cursor-cleanup", "cleanup"),
    )?;
    assert_eq!(inputs_digests(dir.path())?, original);

    // Preset order is part of the inputs: later rules apply over earlier ones.
    let two_presets = MANIFEST
        .replace(
            "[skills.grill-me]",
            "[presets.extra]\nfiles = { exclude = ['x/**'] }\n\n[skills.grill-me]",
        )
        .replace(
            "presets = [\"cursor-cleanup\"]",
            "presets = [\"cursor-cleanup\", \"extra\"]",
        );
    write(dir.path(), "manifest.toml", &two_presets)?;
    let forward = inputs_digests(dir.path())?;
    write(
        dir.path(),
        "manifest.toml",
        &two_presets.replace(
            "presets = [\"cursor-cleanup\", \"extra\"]",
            "presets = [\"extra\", \"cursor-cleanup\"]",
        ),
    )?;
    let backward = inputs_digests(dir.path())?;
    assert_ne!(forward[1], original[1]);
    assert_ne!(backward[1], forward[1]);
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
