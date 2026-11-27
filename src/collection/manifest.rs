//! `manifest.toml`: the user-authored, portable declaration of Sources,
//! Skills, shared D1/D2 rule presets, and Destinations with their Fragments.
//!
//! Parsing rejects malformed names and paths, unknown keys, duplicate keys,
//! and unknown Skill modes or harness types; [`Manifest::problems`] then checks
//! references between entries.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Problem;
use crate::safety::{InstallPath, Name, PathPattern, RelativePath};

/// Collection directory holding written Adaptation and Composition intents.
pub const INTENTS_DIR: &str = "intents";
/// Collection directory holding global instruction Fragments.
pub const FRAGMENTS_DIR: &str = "fragments";

/// Parsed `manifest.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Upstream Git repositories, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<Name, Source>,
    /// Reusable D1/D2 rule sets that Adapt Skills reference by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub presets: BTreeMap<Name, Preset>,
    /// Curated Skills, by the name they are materialised and installed under.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<Name, Skill>,
    /// Portable installation profiles, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub destinations: BTreeMap<Name, Destination>,
}

/// An upstream Git repository from which Skills are fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Repository URL passed to system `git`.
    pub url: String,
    /// Branch whose tip is the candidate Revision on update. Defaults to the
    /// repository's default branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// A named, reusable set of D1 and D2 rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// D1 frontmatter rules.
    #[serde(default, skip_serializing_if = "FrontmatterRules::is_empty")]
    pub frontmatter: FrontmatterRules,
    /// D2 file filtering rules.
    #[serde(default, skip_serializing_if = "FileRules::is_empty")]
    pub files: FileRules,
}

/// D1: deterministic frontmatter rules (set, remove, keep only).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontmatterRules {
    /// Keys set to the given values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, toml::Value>,
    /// Keys removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
    /// When present, the only keys kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_only: Option<Vec<String>>,
}

impl FrontmatterRules {
    /// Whether the rules change nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.remove.is_empty() && self.keep_only.is_none()
    }
}

/// D2: relative path globs selecting the files of an upstream Skill.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRules {
    /// Files matching any of these globs are dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<PathPattern>,
    /// When present, only files matching one of these globs are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<PathPattern>>,
}

impl FileRules {
    /// Whether the rules keep every file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exclude.is_empty() && self.include.is_none()
    }
}

/// A curated Skill, by curation mode (`mode = "..."` in the Manifest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum Skill {
    /// An upstream Skill installed unchanged.
    Copy(CopySkill),
    /// An upstream Skill with an Adaptation.
    Adapt(AdaptSkill),
    /// A Skill produced from several upstream Skills and a written intent.
    Composition(CompositionSkill),
    /// A Skill authored in the Collection under `skills/<name>/`.
    Local(LocalSkill),
}

impl Skill {
    /// Mode name as written in the Manifest.
    #[must_use]
    pub fn mode(&self) -> &'static str {
        match self {
            Self::Copy(_) => "copy",
            Self::Adapt(_) => "adapt",
            Self::Composition(_) => "composition",
            Self::Local(_) => "local",
        }
    }

    /// Path of the written intent, relative to the Collection root.
    #[must_use]
    pub fn intent(&self) -> Option<&RelativePath> {
        match self {
            Self::Adapt(skill) => skill.intent.as_ref(),
            Self::Composition(skill) => skill.intent.as_ref(),
            Self::Copy(_) | Self::Local(_) => None,
        }
    }
}

/// A Skill copied unchanged from one Source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopySkill {
    /// Source the Skill comes from.
    pub source: Name,
    /// Skill directory inside the Source (`.` for the repository root).
    pub upstream_path: RelativePath,
}

/// A Skill adapted from one Source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptSkill {
    /// Source the Skill comes from.
    pub source: Name,
    /// Skill directory inside the Source (`.` for the repository root).
    pub upstream_path: RelativePath,
    /// Presets applied, in order, before the Skill's own rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Name>,
    /// D1 frontmatter rules of this Skill.
    #[serde(default, skip_serializing_if = "FrontmatterRules::is_empty")]
    pub frontmatter: FrontmatterRules,
    /// D2 file filtering rules of this Skill.
    #[serde(default, skip_serializing_if = "FileRules::is_empty")]
    pub files: FileRules,
    /// Written intent for an agent-assisted rewrite, under `intents/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<RelativePath>,
}

/// A Skill composed from several upstream Skills.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositionSkill {
    /// Upstream Skills combined, by input name.
    #[serde(default)]
    pub inputs: BTreeMap<Name, CompositionInput>,
    /// Written intent describing how the inputs combine, under `intents/`.
    /// Required; kept optional here so a missing intent is reported as a
    /// validation problem with guidance rather than a bare parse error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<RelativePath>,
    /// D1 frontmatter invariants enforced on the composed Skill.
    #[serde(default, skip_serializing_if = "FrontmatterRules::is_empty")]
    pub frontmatter: FrontmatterRules,
}

/// One upstream Skill feeding a Composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositionInput {
    /// Source the input comes from.
    pub source: Name,
    /// Skill directory inside the Source (`.` for the repository root).
    pub upstream_path: RelativePath,
    /// D2 file filtering rules of this input.
    #[serde(default, skip_serializing_if = "FileRules::is_empty")]
    pub files: FileRules,
}

/// A Skill authored directly in the Collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSkill {}

/// Harness a Destination installs for. It selects the default Skills
/// directory and install-time transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    /// Claude Code.
    ClaudeCode,
    /// `pi`.
    Pi,
    /// OpenAI Codex.
    Codex,
    /// Antigravity (`agy`).
    Agy,
    /// Any other harness reading Agent Skills directories.
    Generic,
}

impl Harness {
    /// Name as written in the Manifest.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Pi => "pi",
            Self::Codex => "codex",
            Self::Agy => "agy",
            Self::Generic => "generic",
        }
    }

    /// Skills directory used when a Destination does not set `skills_dir`.
    /// `generic` has none.
    #[must_use]
    pub fn default_skills_dir(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => Some("~/.claude/skills"),
            Self::Pi => Some("~/.pi/agent/skills"),
            Self::Codex => Some("~/.agents/skills"),
            Self::Agy => Some("~/.gemini/antigravity-cli/skills"),
            Self::Generic => None,
        }
    }
}

/// Portable installation profile for one harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    /// Harness the Destination installs for.
    pub harness: Harness,
    /// Directory receiving installed Skills. Defaults to the harness's usual
    /// directory; required for `generic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_dir: Option<InstallPath>,
    /// Whether `install` writes to this Destination unless the machine-local
    /// file says otherwise.
    #[serde(default = "enabled_by_default", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Global instruction file assembled from Fragments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Instructions>,
    /// Install-time D1 frontmatter rules applied to every Skill installed
    /// here.
    #[serde(default, skip_serializing_if = "FrontmatterRules::is_empty")]
    pub frontmatter: FrontmatterRules,
}

fn enabled_by_default() -> bool {
    true
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` passes the field by reference"
)]
fn is_true(value: &bool) -> bool {
    *value
}

/// A harness's global instruction file and the Fragments assembled into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instructions {
    /// Global instruction file written at install time, such as
    /// `~/.claude/CLAUDE.md`.
    pub file: InstallPath,
    /// Fragments concatenated in this order, each under `fragments/`.
    pub fragments: Vec<RelativePath>,
}

impl Manifest {
    /// Parses `manifest.toml` text.
    ///
    /// # Errors
    ///
    /// Returns the TOML error, which names the offending line and key, when
    /// the text is not valid TOML or does not match the schema.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Serialises the Manifest as TOML.
    ///
    /// # Errors
    ///
    /// Returns a TOML serialisation error; it cannot occur for a Manifest
    /// built from parsed text.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Reference and consistency problems that parsing alone cannot catch:
    /// undefined Sources and presets, missing intents for Compositions,
    /// intents and Fragments outside their directories, contradictory D1
    /// rules, and Destinations without a Skills directory.
    ///
    /// Does not touch the filesystem.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        for (name, source) in &self.sources {
            check_source(name, source, &mut problems);
        }
        for (name, preset) in &self.presets {
            check_frontmatter(
                &format!("presets.{name}.frontmatter"),
                &preset.frontmatter,
                &mut problems,
            );
        }
        for (name, skill) in &self.skills {
            self.check_skill(name, skill, &mut problems);
        }
        let mut instruction_files = BTreeMap::new();
        for (name, destination) in &self.destinations {
            check_destination(name, destination, &mut problems);
            if let Some(instructions) = &destination.instructions
                && let Some(other) = instruction_files.insert(&instructions.file, name)
            {
                problems.push(Problem::new(
                    format!("destinations.{name}.instructions.file"),
                    format!(
                        "`{}` is already assembled by Destination `{other}`; each global instruction file belongs to one Destination",
                        instructions.file
                    ),
                ));
            }
        }
        problems
    }

    fn check_skill(&self, name: &Name, skill: &Skill, problems: &mut Vec<Problem>) {
        let at = format!("skills.{name}");
        match skill {
            Skill::Copy(copy) => self.check_source_ref(&at, &copy.source, problems),
            Skill::Adapt(adapt) => {
                self.check_source_ref(&at, &adapt.source, problems);
                for preset in &adapt.presets {
                    if !self.presets.contains_key(preset) {
                        problems.push(Problem::new(
                            format!("{at}.presets"),
                            format!(
                                "preset `{preset}` is not declared; add a `[presets.{preset}]` table{}",
                                known(self.presets.keys())
                            ),
                        ));
                    }
                }
                check_frontmatter(&format!("{at}.frontmatter"), &adapt.frontmatter, problems);
                if let Some(intent) = &adapt.intent {
                    check_under(&format!("{at}.intent"), intent, INTENTS_DIR, problems);
                }
            }
            Skill::Composition(composition) => {
                if composition.inputs.len() < 2 {
                    problems.push(Problem::new(
                        format!("{at}.inputs"),
                        format!(
                            "a Composition combines at least two inputs, found {}; use `mode = \"adapt\"` for a single upstream Skill",
                            composition.inputs.len()
                        ),
                    ));
                }
                for (input_name, input) in &composition.inputs {
                    self.check_source_ref(
                        &format!("{at}.inputs.{input_name}"),
                        &input.source,
                        problems,
                    );
                }
                match &composition.intent {
                    Some(intent) => {
                        check_under(&format!("{at}.intent"), intent, INTENTS_DIR, problems);
                    }
                    None => problems.push(Problem::new(
                        at.clone(),
                        format!(
                            "a Composition requires a written intent; add `intent = \"{INTENTS_DIR}/{name}.md\"` and describe in that file how the inputs combine"
                        ),
                    )),
                }
                check_frontmatter(
                    &format!("{at}.frontmatter"),
                    &composition.frontmatter,
                    problems,
                );
            }
            Skill::Local(_) => {}
        }
    }

    fn check_source_ref(&self, at: &str, source: &Name, problems: &mut Vec<Problem>) {
        if !self.sources.contains_key(source) {
            problems.push(Problem::new(
                format!("{at}.source"),
                format!(
                    "Source `{source}` is not declared; add a `[sources.{source}]` table with its `url`{}",
                    known(self.sources.keys())
                ),
            ));
        }
    }
}

/// Lists declared names as a hint, or nothing when there are none.
fn known<'a>(names: impl Iterator<Item = &'a Name>) -> String {
    let names = names.map(|name| format!("`{name}`")).collect::<Vec<_>>();
    if names.is_empty() {
        String::new()
    } else {
        format!(" (declared: {})", names.join(", "))
    }
}

/// Rejects values that `git` could read as a command-line option, and
/// whitespace or control characters.
fn check_git_argument(at: String, what: &str, value: &str, problems: &mut Vec<Problem>) {
    if value.is_empty() {
        problems.push(Problem::new(at, format!("the {what} is empty")));
    } else if value.starts_with('-') {
        problems.push(Problem::new(
            at,
            format!("the {what} must not start with `-`, which `git` would read as an option"),
        ));
    } else if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        problems.push(Problem::new(
            at,
            format!("the {what} must not contain whitespace or control characters"),
        ));
    }
}

fn check_source(name: &Name, source: &Source, problems: &mut Vec<Problem>) {
    check_git_argument(
        format!("sources.{name}.url"),
        "repository URL",
        &source.url,
        problems,
    );
    if let Some(branch) = &source.branch {
        check_git_argument(format!("sources.{name}.branch"), "branch", branch, problems);
    }
}

fn check_frontmatter(at: &str, rules: &FrontmatterRules, problems: &mut Vec<Problem>) {
    for key in &rules.remove {
        if rules.set.contains_key(key) {
            problems.push(Problem::new(
                at.to_owned(),
                format!("key `{key}` is both set and removed; keep only one of the two rules"),
            ));
        }
    }
}

fn check_under(at: &str, path: &RelativePath, dir: &str, problems: &mut Vec<Problem>) {
    if !path.starts_with_dir(dir) {
        problems.push(Problem::new(
            at.to_owned(),
            format!("`{path}` must be a file under `{dir}/`"),
        ));
    }
}

fn check_destination(name: &Name, destination: &Destination, problems: &mut Vec<Problem>) {
    let at = format!("destinations.{name}");
    if destination.skills_dir.is_none() && destination.harness.default_skills_dir().is_none() {
        problems.push(Problem::new(
            format!("{at}.skills_dir"),
            format!(
                "harness `{}` has no default Skills directory; set `skills_dir`",
                destination.harness.as_str()
            ),
        ));
    }
    if let Some(instructions) = &destination.instructions {
        for fragment in &instructions.fragments {
            check_under(
                &format!("{at}.instructions.fragments"),
                fragment,
                FRAGMENTS_DIR,
                problems,
            );
        }
    }
    check_frontmatter(
        &format!("{at}.frontmatter"),
        &destination.frontmatter,
        problems,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Manifest exercising every Skill mode, presets, D1/D2 rules,
    /// Fragments, and every harness type.
    pub(crate) const FULL: &str = r#"
[sources.mattpocock]
url = "https://github.com/mattpocock/skills.git"

[sources.pstack]
url = "https://github.com/cursor/plugins.git"
branch = "main"

[presets.no-auto-invoke.frontmatter]
set = { disable-model-invocation = true }

[presets.cursor-cleanup]
frontmatter = { remove = ["icon", "color"] }
files = { exclude = ["automations/**", "assets/**"] }

[skills.grill-me]
mode = "copy"
source = "mattpocock"
upstream_path = "skills/grill-me"

[skills.grilling]
mode = "adapt"
source = "mattpocock"
upstream_path = "skills/grilling"
presets = ["no-auto-invoke", "cursor-cleanup"]
intent = "intents/grilling.md"

[skills.grilling.frontmatter]
set = { name = "grilling", tags = ["review", "design"], weight = 1.5 }
remove = ["license"]
keep_only = ["name", "description", "disable-model-invocation", "tags", "weight"]

[skills.grilling.files]
exclude = ["*.png"]
include = ["SKILL.md", "references/**"]

[skills.code-review]
mode = "composition"
intent = "intents/code-review.md"

[skills.code-review.inputs.review]
source = "mattpocock"
upstream_path = "skills/review"

[skills.code-review.inputs.critique]
source = "pstack"
upstream_path = "pstack/critique"
files = { exclude = ["automations/**"] }

[skills.code-review.frontmatter]
set = { name = "code-review" }

[skills.fr-learner-voice]
mode = "local"

[destinations.claude]
harness = "claude-code"

[destinations.claude.instructions]
file = "~/.claude/CLAUDE.md"
fragments = ["fragments/git.md", "fragments/claude.md"]

[destinations.pi]
harness = "pi"
skills_dir = "~/.pi/agent/skills"
instructions = { file = "~/.pi/agent/AGENTS.md", fragments = ["fragments/git.md"] }

[destinations.codex]
harness = "codex"
enabled = false
frontmatter = { remove = ["allowed-tools"] }

[destinations.agy]
harness = "agy"
enabled = false

[destinations.shared]
harness = "generic"
skills_dir = "/srv/agents/skills"
"#;

    fn parse(text: &str) -> Manifest {
        Manifest::from_toml(text).unwrap()
    }

    fn parse_err(text: &str) -> String {
        Manifest::from_toml(text).unwrap_err().to_string()
    }

    fn problems(text: &str) -> Vec<String> {
        parse(text)
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn name(raw: &str) -> Name {
        Name::new(raw).unwrap()
    }

    #[test]
    fn parses_every_skill_mode() {
        let manifest = parse(FULL);
        let modes = manifest
            .skills
            .iter()
            .map(|(name, skill)| (name.as_str(), skill.mode()))
            .collect::<Vec<_>>();
        assert_eq!(
            modes,
            [
                ("code-review", "composition"),
                ("fr-learner-voice", "local"),
                ("grill-me", "copy"),
                ("grilling", "adapt"),
            ]
        );
        assert_eq!(manifest.problems(), []);
    }

    #[test]
    fn parses_adapt_rules_and_presets() {
        let manifest = parse(FULL);
        let Some(Skill::Adapt(grilling)) = manifest.skills.get(&name("grilling")) else {
            panic!("grilling is not an Adapt Skill");
        };
        assert_eq!(
            grilling.presets,
            [name("no-auto-invoke"), name("cursor-cleanup")]
        );
        assert_eq!(
            grilling.intent.as_ref().unwrap().as_str(),
            "intents/grilling.md"
        );
        assert_eq!(
            grilling.frontmatter.set.get("tags"),
            Some(&toml::Value::Array(vec!["review".into(), "design".into()]))
        );
        assert_eq!(grilling.frontmatter.remove, ["license"]);
        assert_eq!(grilling.frontmatter.keep_only.as_ref().unwrap().len(), 5);
        assert_eq!(grilling.files.exclude, [PathPattern::new("*.png").unwrap()]);
        assert_eq!(grilling.files.include.as_ref().unwrap().len(), 2);

        let cleanup = &manifest.presets[&name("cursor-cleanup")];
        assert_eq!(cleanup.frontmatter.remove, ["icon", "color"]);
        assert_eq!(cleanup.files.exclude.len(), 2);
        let no_auto = &manifest.presets[&name("no-auto-invoke")];
        assert_eq!(
            no_auto.frontmatter.set.get("disable-model-invocation"),
            Some(&toml::Value::Boolean(true))
        );
        assert!(no_auto.files.is_empty());
    }

    #[test]
    fn parses_compositions() {
        let manifest = parse(FULL);
        let Some(Skill::Composition(review)) = manifest.skills.get(&name("code-review")) else {
            panic!("code-review is not a Composition");
        };
        assert_eq!(review.inputs.len(), 2);
        let critique = &review.inputs[&name("critique")];
        assert_eq!(critique.source, name("pstack"));
        assert_eq!(critique.upstream_path.as_str(), "pstack/critique");
        assert_eq!(critique.files.exclude.len(), 1);
        assert!(review.inputs[&name("review")].files.is_empty());
        assert_eq!(
            review.intent.as_ref().unwrap().as_str(),
            "intents/code-review.md"
        );
    }

    #[test]
    fn parses_destinations_and_fragments() {
        let manifest = parse(FULL);
        let harnesses = manifest
            .destinations
            .values()
            .map(|destination| destination.harness.as_str())
            .collect::<Vec<_>>();
        assert_eq!(harnesses, ["agy", "claude-code", "codex", "pi", "generic"]);

        let claude = &manifest.destinations[&name("claude")];
        assert!(claude.enabled);
        assert_eq!(claude.skills_dir, None);
        let instructions = claude.instructions.as_ref().unwrap();
        assert_eq!(instructions.file.as_str(), "~/.claude/CLAUDE.md");
        let fragments = instructions
            .fragments
            .iter()
            .map(RelativePath::as_str)
            .collect::<Vec<_>>();
        assert_eq!(fragments, ["fragments/git.md", "fragments/claude.md"]);

        let codex = &manifest.destinations[&name("codex")];
        assert!(!codex.enabled);
        assert_eq!(codex.frontmatter.remove, ["allowed-tools"]);
        assert_eq!(
            manifest.destinations[&name("shared")]
                .skills_dir
                .as_ref()
                .unwrap()
                .as_str(),
            "/srv/agents/skills"
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let manifest = parse(FULL);
        let text = manifest.to_toml().unwrap();
        assert_eq!(parse(&text), manifest);
        assert_eq!(parse(&text).to_toml().unwrap(), text);
    }

    #[test]
    fn empty_manifest_is_valid() {
        let manifest = parse("");
        assert_eq!(manifest, Manifest::default());
        assert_eq!(manifest.problems(), []);
        assert_eq!(manifest.to_toml().unwrap(), "");
    }

    #[test]
    fn rejects_path_traversal_and_absolute_paths() {
        let cases = [
            (
                "[skills.x]\nmode = 'copy'\nsource = 's'\nupstream_path = '../x'\n",
                "`..`",
            ),
            (
                "[skills.x]\nmode = 'copy'\nsource = 's'\nupstream_path = '/etc'\n",
                "must be relative",
            ),
            (
                "[skills.x]\nmode = 'adapt'\nsource = 's'\nupstream_path = 'x'\nintent = 'intents/../../x.md'\n",
                "`..`",
            ),
            (
                "[skills.x]\nmode = 'adapt'\nsource = 's'\nupstream_path = 'x'\nfiles = { exclude = ['../**'] }\n",
                "`..`",
            ),
            (
                "[destinations.d]\nharness = 'pi'\nskills_dir = '~/../etc'\n",
                "`..`",
            ),
            (
                "[destinations.d]\nharness = 'pi'\nskills_dir = 'relative/skills'\n",
                "must be absolute or start with `~/`",
            ),
            (
                "[destinations.d]\nharness = 'pi'\ninstructions = { file = '~/A.md', fragments = ['fragments/../../x.md'] }\n",
                "`..`",
            ),
            ("[skills.'../x']\nmode = 'local'\n", "lowercase letter"),
        ];
        for (text, expected) in cases {
            let err = parse_err(text);
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
    }

    #[test]
    fn rejects_invalid_harness_types_and_modes() {
        let err = parse_err("[destinations.d]\nharness = 'cursor'\n");
        assert!(err.contains("unknown variant `cursor`"), "{err}");
        assert!(err.contains("claude-code"), "{err}");

        let err = parse_err("[skills.x]\nmode = 'fork'\n");
        assert!(err.contains("unknown variant `fork`"), "{err}");

        let err = parse_err("[skills.x]\nsource = 's'\n");
        assert!(err.contains("missing field `mode`"), "{err}");
    }

    #[test]
    fn rejects_unknown_and_misplaced_keys() {
        let cases = [
            (
                "[sources.s]\nurl = 'u'\nrevision = 'abc'\n",
                "unknown field `revision`",
            ),
            (
                "[skills.x]\nmode = 'copy'\nsource = 's'\nupstream_path = 'x'\nfiles = { exclude = ['a'] }\n",
                "unknown field `files`",
            ),
            (
                "[skills.x]\nmode = 'local'\nsource = 's'\n",
                "unknown field `source`",
            ),
            (
                "[skills.x]\nmode = 'adapt'\nsource = 's'\nupstream-path = 'x'\n",
                "unknown field `upstream-path`",
            ),
            ("[skillz.x]\nmode = 'local'\n", "unknown field `skillz`"),
        ];
        for (text, expected) in cases {
            let err = parse_err(text);
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
    }

    #[test]
    fn rejects_duplicate_keys() {
        let cases = [
            "[sources.s]\nurl = 'a'\n[sources.s]\nurl = 'b'\n",
            "[skills.x]\nmode = 'local'\n[skills.x]\nmode = 'local'\n",
            "[sources.s]\nurl = 'a'\nurl = 'b'\n",
        ];
        for text in cases {
            let err = parse_err(text);
            assert!(err.contains("duplicate key"), "{text}\n=> {err}");
        }
    }

    #[test]
    fn parse_errors_locate_the_entry_and_name_the_value() {
        // Errors inside a Skill point at its table header, because the
        // `mode` tag is read before the rest of the table.
        let err = parse_err(
            "[sources.s]\nurl = 'u'\n\n[skills.x]\nmode = 'copy'\nsource = 's'\nupstream_path = '../x'\n",
        );
        assert!(err.contains("line 4"), "{err}");
        assert!(err.contains("[skills.x]"), "{err}");
        assert!(
            err.contains("\"../x\" is not a valid relative path"),
            "{err}"
        );

        let err = parse_err(
            "[sources.s]\nurl = 'u'\n\n[destinations.d]\nharness = 'pi'\nskills_dir = 'skills'\n",
        );
        assert!(err.contains("line 6"), "{err}");
        assert!(
            err.contains("\"skills\" is not a valid installation path"),
            "{err}"
        );
    }

    #[test]
    fn reports_undefined_sources_and_presets() {
        let found = problems(
            "[sources.known]\nurl = 'u'\n[skills.a]\nmode = 'copy'\nsource = 'missing'\nupstream_path = 'a'\n\
             [skills.b]\nmode = 'adapt'\nsource = 'known'\nupstream_path = 'b'\npresets = ['nope']\n",
        );
        assert_eq!(
            found,
            [
                "skills.a.source: Source `missing` is not declared; add a `[sources.missing]` table with its `url` (declared: `known`)",
                "skills.b.presets: preset `nope` is not declared; add a `[presets.nope]` table",
            ]
        );
    }

    #[test]
    fn reports_undefined_composition_input_sources() {
        let found = problems(
            "[sources.s]\nurl = 'u'\n[skills.c]\nmode = 'composition'\nintent = 'intents/c.md'\n\
             [skills.c.inputs.a]\nsource = 's'\nupstream_path = 'a'\n\
             [skills.c.inputs.b]\nsource = 'gone'\nupstream_path = 'b'\n",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("skills.c.inputs.b.source: Source `gone`"),
            "{found:?}"
        );
    }

    #[test]
    fn reports_compositions_without_intent_or_enough_inputs() {
        let found = problems(
            "[sources.s]\nurl = 'u'\n[skills.c]\nmode = 'composition'\n[skills.c.inputs.a]\nsource = 's'\nupstream_path = 'a'\n",
        );
        assert_eq!(
            found,
            [
                "skills.c.inputs: a Composition combines at least two inputs, found 1; use `mode = \"adapt\"` for a single upstream Skill",
                "skills.c: a Composition requires a written intent; add `intent = \"intents/c.md\"` and describe in that file how the inputs combine",
            ]
        );
    }

    #[test]
    fn reports_intents_and_fragments_outside_their_directories() {
        let found = problems(
            "[sources.s]\nurl = 'u'\n[skills.a]\nmode = 'adapt'\nsource = 's'\nupstream_path = 'a'\nintent = 'skills/a/SKILL.md'\n\
             [destinations.d]\nharness = 'pi'\ninstructions = { file = '~/AGENTS.md', fragments = ['README.md'] }\n",
        );
        assert_eq!(
            found,
            [
                "skills.a.intent: `skills/a/SKILL.md` must be a file under `intents/`",
                "destinations.d.instructions.fragments: `README.md` must be a file under `fragments/`",
            ]
        );
    }

    #[test]
    fn reports_git_arguments_that_could_be_options() {
        let found = problems(
            "[sources.a]\nurl = '--upload-pack=touch /tmp/x'\n[sources.b]\nurl = 'u'\nbranch = '-x'\n[sources.c]\nurl = ''\n[sources.d]\nurl = 'a b'\n",
        );
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(found[0].contains("sources.a.url") && found[0].contains("`-`"));
        assert!(found[1].contains("sources.b.branch") && found[1].contains("`-`"));
        assert!(found[2].contains("sources.c.url") && found[2].contains("empty"));
        assert!(found[3].contains("sources.d.url") && found[3].contains("whitespace"));
    }

    #[test]
    fn reports_contradictory_frontmatter_rules() {
        let found = problems("[presets.p.frontmatter]\nset = { icon = 'x' }\nremove = ['icon']\n");
        assert_eq!(
            found,
            [
                "presets.p.frontmatter: key `icon` is both set and removed; keep only one of the two rules"
            ]
        );
    }

    #[test]
    fn reports_generic_destinations_without_a_skills_dir() {
        let found = problems("[destinations.g]\nharness = 'generic'\n");
        assert_eq!(
            found,
            [
                "destinations.g.skills_dir: harness `generic` has no default Skills directory; set `skills_dir`"
            ]
        );
    }

    #[test]
    fn reports_instruction_files_shared_by_destinations() {
        let found = problems(
            "[destinations.a]\nharness = 'pi'\ninstructions = { file = '~/AGENTS.md', fragments = [] }\n\
             [destinations.b]\nharness = 'codex'\ninstructions = { file = '~//AGENTS.md', fragments = [] }\n",
        );
        assert_eq!(
            found,
            [
                "destinations.b.instructions.file: `~/AGENTS.md` is already assembled by Destination `a`; each global instruction file belongs to one Destination"
            ]
        );
    }

    #[test]
    fn harness_defaults_follow_the_spec() {
        assert_eq!(
            Harness::ClaudeCode.default_skills_dir(),
            Some("~/.claude/skills")
        );
        assert_eq!(Harness::Pi.default_skills_dir(), Some("~/.pi/agent/skills"));
        assert_eq!(
            Harness::Codex.default_skills_dir(),
            Some("~/.agents/skills")
        );
        assert_eq!(
            Harness::Agy.default_skills_dir(),
            Some("~/.gemini/antigravity-cli/skills")
        );
        assert_eq!(Harness::Generic.default_skills_dir(), None);
        for default in [
            Harness::ClaudeCode,
            Harness::Pi,
            Harness::Codex,
            Harness::Agy,
        ]
        .map(Harness::default_skills_dir)
        .into_iter()
        .flatten()
        {
            InstallPath::new(default).unwrap();
        }
    }
}
