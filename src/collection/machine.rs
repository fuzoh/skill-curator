//! `machine.toml`: optional, uncommitted, machine-local settings that enable,
//! disable, or relocate the Manifest's portable Destinations on one machine.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Problem;
use super::manifest::{FrontmatterRules, Harness, Manifest};
use crate::safety::{HomeDir, InstallPath, Name, RelativePath};

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
    /// `collection_roots` lists the absolute forms of the Collection root
    /// (for example canonical and as given); no Destination path may be,
    /// lie inside, or contain any of them.
    ///
    /// # Errors
    ///
    /// Returns the problems found: settings for a Destination the Manifest
    /// does not declare, an instruction file for a Destination without
    /// instructions, a missing Skills directory, a path that is the home
    /// directory or one of its parents, a path that is, lies inside, or
    /// contains the Collection root, or enabled Destinations whose Skills
    /// directories and instruction files are the same or inside one another.
    pub fn resolve(
        &self,
        manifest: &Manifest,
        home: &HomeDir,
        collection_roots: &[PathBuf],
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

        check_paths(&resolved, home, collection_roots, &mut problems);
        if problems.is_empty() {
            Ok(resolved)
        } else {
            Err(problems)
        }
    }
}

/// Reports Destination paths that would put managed files among unrelated
/// ones: any path that is the home directory or one of its parents, any path
/// that is, lies inside, or contains the Collection root, and, across enabled
/// Destinations, Skills directories and instruction files that are the same
/// or inside one another, so that no install writes into the Collection or
/// another Destination's tree.
fn check_paths(
    resolved: &[ResolvedDestination],
    home: &HomeDir,
    collection_roots: &[PathBuf],
    problems: &mut Vec<Problem>,
) {
    let mut written: Vec<(&Name, &str, &Path)> = Vec::new();
    for destination in resolved {
        let at = format!("destinations.{}", destination.name);
        let instruction_file = destination
            .instructions
            .as_ref()
            .map(|instructions| ("instruction file", instructions.file.as_path()));
        let paths = [("Skills directory", destination.skills_dir.as_path())]
            .into_iter()
            .chain(instruction_file);
        for (what, path) in paths {
            let shown = path.display();
            if home.as_path().starts_with(path) {
                problems.push(Problem::new(
                    at.clone(),
                    format!(
                        "its {what} {shown} is the home directory or one of its parents; choose a dedicated path below the home directory"
                    ),
                ));
            }
            // One report per path, even when the root has several forms.
            let collection_clash = collection_roots.iter().find_map(|root| {
                let relation = if path == root {
                    "is"
                } else if path.starts_with(root) {
                    "is inside"
                } else if root.starts_with(path) {
                    "contains"
                } else {
                    return None;
                };
                Some((relation, root.display()))
            });
            if let Some((relation, root)) = collection_clash {
                problems.push(Problem::new(
                    at.clone(),
                    format!(
                        "its {what} {shown} {relation} the Collection root {root}; choose a path outside the Collection"
                    ),
                ));
            }
            if !destination.enabled {
                continue;
            }
            for &(other, other_what, other_path) in &written {
                let owner = if *other == destination.name {
                    format!("its own {other_what}")
                } else {
                    format!("the {other_what} of enabled Destination `{other}`")
                };
                let other_shown = other_path.display();
                let relation = if path == other_path {
                    format!("is also {owner}")
                } else if path.starts_with(other_path) {
                    format!("is inside {other_shown}, {owner}")
                } else if other_path.starts_with(path) {
                    format!("contains {other_shown}, {owner}")
                } else {
                    continue;
                };
                problems.push(Problem::new(
                    at.clone(),
                    format!(
                        "its {what} {shown} {relation}; give each enabled Destination its own paths, none inside another"
                    ),
                ));
            }
            written.push((&destination.name, what, path));
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
    const COLLECTION: &str = "/home/user/lab/collection";

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

    fn resolve_with(
        manifest: &Manifest,
        machine: &str,
    ) -> Result<Vec<ResolvedDestination>, Vec<String>> {
        MachineConfig::from_toml(machine)
            .unwrap()
            .resolve(
                manifest,
                &HomeDir::new(HOME).unwrap(),
                &[PathBuf::from(COLLECTION)],
            )
            .map_err(|problems| problems.iter().map(ToString::to_string).collect())
    }

    fn resolve(machine: &str) -> Result<Vec<ResolvedDestination>, Vec<String>> {
        resolve_with(&manifest(), machine)
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
        assert_eq!(
            problems,
            [
                "destinations.pi: its instruction file /home/user/.claude/CLAUDE.md is also the instruction file of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
                "destinations.shared: its Skills directory /home/user/.claude/skills is also the Skills directory of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
            ]
        );

        // A disabled Destination may share paths: it writes nothing.
        assert!(resolve("[destinations.agy]\nskills_dir = '~/.claude/skills'\n").is_ok());
    }

    #[test]
    fn reports_enabled_destination_paths_inside_one_another() {
        // A Skills directory inside another Destination's Skills directory,
        // in both declaration orders.
        let problems = resolve("[destinations.shared]\nskills_dir = '~/.claude'\n").unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.shared: its Skills directory /home/user/.claude contains /home/user/.claude/skills, the Skills directory of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
                "destinations.shared: its Skills directory /home/user/.claude contains /home/user/.claude/CLAUDE.md, the instruction file of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
            ]
        );
        let problems =
            resolve("[destinations.shared]\nskills_dir = '~/.claude/skills/nested'\n").unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.shared: its Skills directory /home/user/.claude/skills/nested is inside /home/user/.claude/skills, the Skills directory of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
            ]
        );

        // An instruction file inside another Destination's Skills directory,
        // or inside the Destination's own.
        let problems =
            resolve("[destinations.pi]\ninstruction_file = '~/.claude/skills/x.md'\n").unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.pi: its instruction file /home/user/.claude/skills/x.md is inside /home/user/.claude/skills, the Skills directory of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
            ]
        );
        let problems =
            resolve("[destinations.pi]\ninstruction_file = '~/.pi/agent/skills/AGENTS.md'\n")
                .unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.pi: its instruction file /home/user/.pi/agent/skills/AGENTS.md is inside /home/user/.pi/agent/skills, its own Skills directory; give each enabled Destination its own paths, none inside another",
            ]
        );

        // Sibling paths with a common name prefix are not nested.
        assert!(resolve("[destinations.shared]\nskills_dir = '~/.claude/skills-extra'\n").is_ok());
        // A disabled Destination may nest: it writes nothing.
        assert!(resolve("[destinations.agy]\nskills_dir = '~/.claude'\n").is_ok());
    }

    #[test]
    fn reports_paths_at_or_above_the_home_directory() {
        let problems = resolve(
            "[destinations.shared]\nskills_dir = '/home/user'\n[destinations.agy]\nskills_dir = '/home'\n",
        )
        .unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.agy: its Skills directory /home is the home directory or one of its parents; choose a dedicated path below the home directory",
                "destinations.agy: its Skills directory /home contains the Collection root /home/user/lab/collection; choose a path outside the Collection",
                "destinations.shared: its Skills directory /home/user is the home directory or one of its parents; choose a dedicated path below the home directory",
                "destinations.shared: its Skills directory /home/user contains the Collection root /home/user/lab/collection; choose a path outside the Collection",
                "destinations.shared: its Skills directory /home/user contains /home/user/.claude/skills, the Skills directory of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
                "destinations.shared: its Skills directory /home/user contains /home/user/.claude/CLAUDE.md, the instruction file of enabled Destination `claude`; give each enabled Destination its own paths, none inside another",
                "destinations.shared: its Skills directory /home/user contains /home/user/.pi/agent/skills, the Skills directory of enabled Destination `pi`; give each enabled Destination its own paths, none inside another",
                "destinations.shared: its Skills directory /home/user contains /home/user/.pi/agent/AGENTS.md, the instruction file of enabled Destination `pi`; give each enabled Destination its own paths, none inside another",
            ]
        );
        let problems = resolve("[destinations.pi]\ninstruction_file = '/home/user'\n").unwrap_err();
        assert!(
            problems[0].starts_with(
                "destinations.pi: its instruction file /home/user is the home directory or one of its parents"
            ),
            "{problems:?}"
        );
        // `~` and `/` alone are rejected when parsing.
        for path in ["~", "/"] {
            let err =
                MachineConfig::from_toml(&format!("[destinations.pi]\nskills_dir = '{path}'\n"))
                    .unwrap_err()
                    .to_string();
            assert!(
                err.contains("not the filesystem root or the home directory itself"),
                "{err}"
            );
        }
    }

    #[test]
    fn reports_paths_that_are_inside_or_contain_the_collection() {
        for (machine, expected) in [
            (
                "[destinations.shared]\nskills_dir = '~/lab/collection/skills'\n",
                "destinations.shared: its Skills directory /home/user/lab/collection/skills is inside the Collection root /home/user/lab/collection; choose a path outside the Collection",
            ),
            (
                "[destinations.claude]\ninstruction_file = '~/lab/collection/manifest.toml'\n",
                "destinations.claude: its instruction file /home/user/lab/collection/manifest.toml is inside the Collection root /home/user/lab/collection; choose a path outside the Collection",
            ),
            (
                "[destinations.pi]\nskills_dir = '~/lab'\n",
                "destinations.pi: its Skills directory /home/user/lab contains the Collection root /home/user/lab/collection; choose a path outside the Collection",
            ),
            (
                "[destinations.shared]\nskills_dir = '~/lab/collection'\n",
                "destinations.shared: its Skills directory /home/user/lab/collection is the Collection root /home/user/lab/collection; choose a path outside the Collection",
            ),
        ] {
            assert_eq!(resolve(machine).unwrap_err(), [expected], "{machine}");
        }

        // Every form of the root is checked, each path reported once.
        let manifest = manifest();
        let home = HomeDir::new(HOME).unwrap();
        let roots = [
            PathBuf::from("/srv/real/collection"),
            PathBuf::from(COLLECTION),
        ];
        let problems = MachineConfig::from_toml(
            "[destinations.shared]\nskills_dir = '/srv/real/collection/skills'\n\
             [destinations.pi]\nskills_dir = '~/lab/collection/skills'\n",
        )
        .unwrap()
        .resolve(&manifest, &home, &roots)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        assert_eq!(
            problems,
            [
                "destinations.pi: its Skills directory /home/user/lab/collection/skills is inside the Collection root /home/user/lab/collection; choose a path outside the Collection",
                "destinations.shared: its Skills directory /srv/real/collection/skills is inside the Collection root /srv/real/collection; choose a path outside the Collection",
            ]
        );
        let problems = MachineConfig::from_toml(
            "[destinations.shared]\nskills_dir = '/srv/real/collection'\n",
        )
        .unwrap()
        .resolve(
            &manifest,
            &home,
            &[
                PathBuf::from("/srv/real/collection"),
                PathBuf::from("/srv/real/collection"),
            ],
        )
        .unwrap_err();
        assert_eq!(problems.len(), 1, "{problems:?}");

        // Disabled Destinations are checked too; siblings with a common name
        // prefix are not nested.
        assert!(resolve("[destinations.agy]\nskills_dir = '~/lab/collection/x'\n").is_err());
        assert!(resolve("[destinations.shared]\nskills_dir = '~/lab/collection-skills'\n").is_ok());
    }

    #[test]
    fn machines_may_each_enable_one_of_two_destinations_sharing_a_file() {
        let manifest = Manifest::from_toml(
            "[destinations.laptop]\nharness = 'claude-code'\ninstructions = { file = '~/.claude/CLAUDE.md', fragments = ['fragments/a.md'] }\n\
             [destinations.server]\nharness = 'generic'\nskills_dir = '~/srv-skills'\nenabled = false\ninstructions = { file = '~/.claude/CLAUDE.md', fragments = ['fragments/b.md'] }\n",
        )
        .unwrap();
        assert!(resolve_with(&manifest, "").is_ok());
        assert!(
            resolve_with(
                &manifest,
                "[destinations.laptop]\nenabled = false\n[destinations.server]\nenabled = true\n"
            )
            .is_ok()
        );
        let problems =
            resolve_with(&manifest, "[destinations.server]\nenabled = true\n").unwrap_err();
        assert_eq!(
            problems,
            [
                "destinations.server: its instruction file /home/user/.claude/CLAUDE.md is also the instruction file of enabled Destination `laptop`; give each enabled Destination its own paths, none inside another",
            ]
        );
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
