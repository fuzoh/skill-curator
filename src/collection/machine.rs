//! `machine.toml`: optional, uncommitted, machine-local settings that enable,
//! disable, or relocate the Manifest's portable Destinations on one machine.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Problem;
use super::manifest::{FrontmatterRules, Harness, Manifest};
use crate::safety::{InstallPath, Name, RelativePath};

/// Parsed `machine.toml`. An absent file is the default: every Destination
/// keeps its portable settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    /// Machine-local settings per Destination, by Destination name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub destinations: BTreeMap<Name, MachineDestination>,
}

/// Machine-local settings for one Destination. Each present field replaces
/// the portable value from the Manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineDestination {
    /// Whether `install` writes to the Destination on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Skills directory on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_dir: Option<InstallPath>,
    /// Global instruction file on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_file: Option<InstallPath>,
}

/// A Destination as it applies on this machine: portable profile merged with
/// machine-local settings, paths expanded.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDestination {
    /// Destination name.
    pub name: Name,
    /// Harness the Destination installs for.
    pub harness: Harness,
    /// Whether `install` writes to it on this machine.
    pub enabled: bool,
    /// Directory receiving installed Skills.
    pub skills_dir: PathBuf,
    /// Global instruction file and its Fragments, if any.
    pub instructions: Option<ResolvedInstructions>,
    /// Install-time D1 frontmatter rules.
    pub frontmatter: FrontmatterRules,
}

/// A global instruction file on this machine and its ordered Fragments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInstructions {
    /// Global instruction file.
    pub file: PathBuf,
    /// Fragments concatenated in this order, relative to the Collection root.
    pub fragments: Vec<RelativePath>,
}

impl MachineConfig {
    /// Parses `machine.toml` text.
    ///
    /// # Errors
    ///
    /// Returns the TOML error when the text is not valid TOML or does not
    /// match the schema.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Serialises the settings as TOML.
    ///
    /// # Errors
    ///
    /// Returns a TOML serialisation error; it cannot occur for settings built
    /// from parsed text.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Merges these machine-local settings over the Manifest's portable
    /// Destinations, expanding `~` to `home`.
    ///
    /// # Errors
    ///
    /// Returns the problems found: settings for a Destination the Manifest
    /// does not declare, an instruction file for a Destination without
    /// instructions, a missing Skills directory, or enabled Destinations that
    /// would write to the same directory or instruction file.
    pub fn resolve(
        &self,
        manifest: &Manifest,
        home: &Path,
    ) -> Result<Vec<ResolvedDestination>, Vec<Problem>> {
        let mut problems = Vec::new();
        for name in self.destinations.keys() {
            if !manifest.destinations.contains_key(name) {
                problems.push(Problem::new(
                    format!("destinations.{name}"),
                    format!(
                        "Destination `{name}` is not declared in manifest.toml; remove these settings or declare the Destination"
                    ),
                ));
            }
        }

        let mut resolved = Vec::new();
        for (name, portable) in &manifest.destinations {
            let local = self.destinations.get(name).cloned().unwrap_or_default();
            let skills_dir = local
                .skills_dir
                .or_else(|| portable.skills_dir.clone())
                .or_else(|| {
                    portable
                        .harness
                        .default_skills_dir()
                        .and_then(|default| InstallPath::new(default).ok())
                });
            let Some(skills_dir) = skills_dir else {
                problems.push(Problem::new(
                    format!("destinations.{name}.skills_dir"),
                    format!(
                        "harness `{}` has no default Skills directory; set `skills_dir`",
                        portable.harness.as_str()
                    ),
                ));
                continue;
            };
            let instructions = match (&portable.instructions, local.instruction_file) {
                (Some(instructions), file) => Some(ResolvedInstructions {
                    file: file
                        .unwrap_or_else(|| instructions.file.clone())
                        .expand(home),
                    fragments: instructions.fragments.clone(),
                }),
                (None, Some(_)) => {
                    problems.push(Problem::new(
                        format!("destinations.{name}.instruction_file"),
                        format!(
                            "Destination `{name}` declares no `instructions` in manifest.toml, so there is no instruction file to relocate"
                        ),
                    ));
                    None
                }
                (None, None) => None,
            };
            resolved.push(ResolvedDestination {
                name: name.clone(),
                harness: portable.harness,
                enabled: local.enabled.unwrap_or(portable.enabled),
                skills_dir: skills_dir.expand(home),
                instructions,
                frontmatter: portable.frontmatter.clone(),
            });
        }

        check_shared_paths(&resolved, &mut problems);
        if problems.is_empty() {
            Ok(resolved)
        } else {
            Err(problems)
        }
    }
}

/// Reports enabled Destinations that would write to the same Skills
/// directory or global instruction file.
fn check_shared_paths(resolved: &[ResolvedDestination], problems: &mut Vec<Problem>) {
    let mut owners: BTreeMap<&Path, &Name> = BTreeMap::new();
    for destination in resolved.iter().filter(|destination| destination.enabled) {
        let instruction_file = destination
            .instructions
            .as_ref()
            .map(|instructions| ("instruction file", instructions.file.as_path()));
        let paths = [("Skills directory", destination.skills_dir.as_path())]
            .into_iter()
            .chain(instruction_file);
        for (what, path) in paths {
            if let Some(other) = owners.insert(path, &destination.name) {
                problems.push(Problem::new(
                    format!("destinations.{}", destination.name),
                    format!(
                        "its {what} {} is also written by enabled Destination `{other}`; give each enabled Destination its own paths",
                        path.display()
                    ),
                ));
            }
        }
    }
}

/// Default location of `machine.toml`: `$XDG_CONFIG_HOME/skill-curator/` when
/// that variable holds an absolute path, `$HOME/.config/skill-curator/`
/// otherwise.
///
/// Returns `None` when neither variable gives an absolute directory.
#[must_use]
pub fn default_machine_config_path(
    xdg_config_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    let config_home = absolute_dir(xdg_config_home)
        .map(Path::to_path_buf)
        .or_else(|| absolute_dir(home).map(|home| home.join(".config")))?;
    Some(config_home.join("skill-curator").join("machine.toml"))
}

fn absolute_dir(value: Option<&OsStr>) -> Option<&Path> {
    value.map(Path::new).filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/user";

    const MANIFEST: &str = r#"
[destinations.claude]
harness = "claude-code"
instructions = { file = "~/.claude/CLAUDE.md", fragments = ["fragments/git.md", "fragments/claude.md"] }
frontmatter = { remove = ["icon"] }

[destinations.pi]
harness = "pi"
instructions = { file = "~/.pi/agent/AGENTS.md", fragments = ["fragments/git.md"] }

[destinations.agy]
harness = "agy"
enabled = false

[destinations.shared]
harness = "generic"
skills_dir = "/srv/agents/skills"
"#;

    fn manifest() -> Manifest {
        Manifest::from_toml(MANIFEST).unwrap()
    }

    fn resolve(machine: &str) -> Result<Vec<ResolvedDestination>, Vec<String>> {
        MachineConfig::from_toml(machine)
            .unwrap()
            .resolve(&manifest(), Path::new(HOME))
            .map_err(|problems| problems.iter().map(ToString::to_string).collect())
    }

    fn by_name<'a>(resolved: &'a [ResolvedDestination], name: &str) -> &'a ResolvedDestination {
        resolved
            .iter()
            .find(|destination| destination.name.as_str() == name)
            .unwrap()
    }

    #[test]
    fn parses_and_round_trips() {
        let text = r#"
[destinations.claude]
enabled = false

[destinations.pi]
skills_dir = "/opt/pi/skills"
instruction_file = "~/pi/AGENTS.md"
"#;
        let machine = MachineConfig::from_toml(text).unwrap();
        let pi = &machine.destinations[&Name::new("pi").unwrap()];
        assert_eq!(pi.enabled, None);
        assert_eq!(pi.skills_dir.as_ref().unwrap().as_str(), "/opt/pi/skills");
        let claude = &machine.destinations[&Name::new("claude").unwrap()];
        assert_eq!(claude.enabled, Some(false));

        let serialised = machine.to_toml().unwrap();
        assert_eq!(MachineConfig::from_toml(&serialised).unwrap(), machine);
        assert_eq!(MachineConfig::default().to_toml().unwrap(), "");
    }

    #[test]
    fn rejects_unknown_keys_and_unsafe_paths() {
        for (text, expected) in [
            (
                "[destinations.pi]\nharness = 'codex'\n",
                "unknown field `harness`",
            ),
            ("[destinations.pi]\nskills_dir = '~/../x'\n", "`..`"),
            (
                "[destinations.pi]\ninstruction_file = 'AGENTS.md'\n",
                "`~/`",
            ),
            ("[destinations.pi]\nenabled = 'no'\n", "expected a boolean"),
            ("[hosts.bnd]\n", "unknown field `hosts`"),
        ] {
            let err = MachineConfig::from_toml(text).unwrap_err().to_string();
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
    }

    #[test]
    fn missing_settings_keep_portable_defaults() {
        let resolved = resolve("").unwrap();
        let names = resolved
            .iter()
            .map(|destination| destination.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["agy", "claude", "pi", "shared"]);

        let claude = by_name(&resolved, "claude");
        assert!(claude.enabled);
        assert_eq!(claude.harness, Harness::ClaudeCode);
        assert_eq!(claude.skills_dir, Path::new("/home/user/.claude/skills"));
        let instructions = claude.instructions.as_ref().unwrap();
        assert_eq!(instructions.file, Path::new("/home/user/.claude/CLAUDE.md"));
        assert_eq!(instructions.fragments.len(), 2);
        assert_eq!(claude.frontmatter.remove, ["icon"]);

        let agy = by_name(&resolved, "agy");
        assert!(!agy.enabled);
        assert_eq!(
            agy.skills_dir,
            Path::new("/home/user/.gemini/antigravity-cli/skills")
        );
        assert_eq!(agy.instructions, None);

        assert_eq!(
            by_name(&resolved, "pi").skills_dir,
            Path::new("/home/user/.pi/agent/skills")
        );
        assert_eq!(
            by_name(&resolved, "shared").skills_dir,
            Path::new("/srv/agents/skills")
        );
    }

    #[test]
    fn machine_settings_replace_portable_values() {
        let resolved = resolve(
            r#"
[destinations.claude]
enabled = false

[destinations.agy]
enabled = true

[destinations.pi]
skills_dir = "/opt/pi/skills"
instruction_file = "~/work/AGENTS.md"
"#,
        )
        .unwrap();
        assert!(!by_name(&resolved, "claude").enabled);
        assert!(by_name(&resolved, "agy").enabled);
        let pi = by_name(&resolved, "pi");
        assert!(pi.enabled);
        assert_eq!(pi.harness, Harness::Pi);
        assert_eq!(pi.skills_dir, Path::new("/opt/pi/skills"));
        let instructions = pi.instructions.as_ref().unwrap();
        assert_eq!(instructions.file, Path::new("/home/user/work/AGENTS.md"));
        assert_eq!(instructions.fragments[0].as_str(), "fragments/git.md");
        // Untouched Destinations keep their portable values.
        assert_eq!(
            by_name(&resolved, "shared").skills_dir,
            Path::new("/srv/agents/skills")
        );
    }

    #[test]
    fn reports_settings_for_undeclared_destinations() {
        let problems = resolve("[destinations.cursor]\nenabled = true\n").unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.cursor: Destination `cursor` is not declared in manifest.toml; remove these settings or declare the Destination"
            ]
        );
    }

    #[test]
    fn reports_instruction_files_without_instructions() {
        let problems =
            resolve("[destinations.shared]\ninstruction_file = '~/AGENTS.md'\n").unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.shared.instruction_file: Destination `shared` declares no `instructions` in manifest.toml, so there is no instruction file to relocate"
            ]
        );
    }

    #[test]
    fn reports_enabled_destinations_sharing_paths() {
        let problems = resolve(
            "[destinations.shared]\nskills_dir = '~/.claude/skills'\n[destinations.pi]\ninstruction_file = '~/.claude/CLAUDE.md'\n",
        )
        .unwrap_err();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("destinations.pi: its instruction file /home/user/.claude/CLAUDE.md is also written by enabled Destination `claude`"));
        assert!(problems[1].starts_with("destinations.shared: its Skills directory /home/user/.claude/skills is also written by enabled Destination `claude`"));

        // A disabled Destination may share paths: it writes nothing.
        assert!(resolve("[destinations.agy]\nskills_dir = '~/.claude/skills'\n").is_ok());
    }

    #[test]
    fn default_path_follows_xdg_then_home() {
        let path = |xdg: Option<&str>, home: Option<&str>| {
            default_machine_config_path(xdg.map(OsStr::new), home.map(OsStr::new))
        };
        assert_eq!(
            path(Some("/xdg"), Some("/home/u")),
            Some(PathBuf::from("/xdg/skill-curator/machine.toml"))
        );
        assert_eq!(
            path(None, Some("/home/u")),
            Some(PathBuf::from("/home/u/.config/skill-curator/machine.toml"))
        );
        assert_eq!(
            path(Some(""), Some("/home/u")),
            Some(PathBuf::from("/home/u/.config/skill-curator/machine.toml"))
        );
        assert_eq!(
            path(Some("relative"), Some("/home/u")),
            Some(PathBuf::from("/home/u/.config/skill-curator/machine.toml"))
        );
        assert_eq!(path(None, None), None);
        assert_eq!(path(None, Some("relative")), None);
    }
}
