//! Install-time deterministic transforms: what an accepted Skill becomes in
//! one Destination.
//!
//! Every harness applies the Destination's own D1 frontmatter rules to
//! `SKILL.md`. A `codex` Destination also translates
//! `disable-model-invocation: true`, which Codex does not read, into
//! `agents/openai.yaml` with `policy.allow_implicit_invocation: false`, and
//! warns when a Skill Codex may invoke implicitly has a description longer
//! than Codex lists by default.

use std::fmt;

use crate::adaptation::frontmatter::{Entry, FrontmatterError, Node, SkillDocument, apply_rules};
use crate::adaptation::{AdaptationError, SkillFile, SkillTree};
use crate::collection::manifest::{FrontmatterRules, Harness};
use crate::safety::RelativePath;

/// Codex's per-Skill metadata file, relative to the Skill directory.
pub const CODEX_METADATA_FILE: &str = "agents/openai.yaml";
/// Contents of [`CODEX_METADATA_FILE`] for a Skill only the user may invoke.
pub const CODEX_MANUAL_ONLY_POLICY: &str = "policy:\n  allow_implicit_invocation: false\n";
/// Frontmatter key Claude Code reads to keep the model from invoking a Skill.
pub const DISABLE_MODEL_INVOCATION: &str = "disable-model-invocation";
/// Characters of Skill descriptions Codex lists by default.
pub const CODEX_DESCRIPTION_LIMIT: usize = 8_000;

/// A Skill as installed in one Destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapted {
    /// The files to install.
    pub tree: SkillTree,
    /// Warnings about the installed Skill, without its name.
    pub warnings: Vec<String>,
}

/// Why a Skill cannot be installed in a Destination.
#[derive(Debug)]
#[non_exhaustive]
pub enum HarnessError {
    /// `SKILL.md` cannot be read, or the Destination's D1 rules cannot be
    /// applied to it.
    Adaptation(AdaptationError),
    /// The Skill already ships the Codex metadata file that the translation
    /// of `disable-model-invocation` would write.
    MetadataFileExists,
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adaptation(err) => err.fmt(f),
            Self::MetadataFileExists => write!(
                f,
                "the Skill sets `{DISABLE_MODEL_INVOCATION}: true` and already has `{CODEX_METADATA_FILE}`, which skill-curator does not merge; remove that file from the Skill or set `allow_implicit_invocation: false` in it yourself and drop `{DISABLE_MODEL_INVOCATION}`"
            ),
        }
    }
}

impl std::error::Error for HarnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Adaptation(err) => Some(err),
            Self::MetadataFileExists => None,
        }
    }
}

impl From<AdaptationError> for HarnessError {
    fn from(err: AdaptationError) -> Self {
        Self::Adaptation(err)
    }
}

impl From<FrontmatterError> for HarnessError {
    fn from(err: FrontmatterError) -> Self {
        Self::Adaptation(err.into())
    }
}

/// The accepted Skill `skill` as installed in a Destination for `harness`
/// with the install-time D1 `rules`.
///
/// The Codex translation reads the accepted `SKILL.md`, before the
/// Destination's rules, so those rules may strip
/// `disable-model-invocation` (which Codex's validator rejects) without
/// losing the translation.
///
/// # Errors
///
/// Returns a [`HarnessError`] when `SKILL.md` is missing, not UTF-8, or has
/// a frontmatter the rules (or, for Codex, the translation) cannot read, or
/// when a Codex translation would overwrite a file the Skill ships.
pub fn adapt(
    harness: Harness,
    rules: &FrontmatterRules,
    skill: &SkillTree,
) -> Result<Adapted, HarnessError> {
    let accepted = skill.skill_md()?;
    let manual_only = match harness {
        Harness::Codex => is_true(SkillDocument::parse(accepted)?.entry(DISABLE_MODEL_INVOCATION)),
        Harness::ClaudeCode | Harness::Pi | Harness::Agy | Harness::Generic => false,
    };
    let text = apply_rules(accepted, &[rules])?;
    let mut tree = skill.clone();
    let mut warnings = Vec::new();
    if harness == Harness::Codex {
        let metadata = RelativePath::exact(CODEX_METADATA_FILE).map_err(|reason| {
            AdaptationError::UnsafePath {
                path: CODEX_METADATA_FILE.to_owned(),
                reason,
            }
        })?;
        if manual_only {
            if tree.get(&metadata).is_some() {
                return Err(HarnessError::MetadataFileExists);
            }
            tree.insert(
                metadata,
                SkillFile {
                    contents: CODEX_MANUAL_ONLY_POLICY.as_bytes().to_vec(),
                    executable: false,
                },
            );
        } else if let Some(length) = description_length(&text)?
            && length > CODEX_DESCRIPTION_LIMIT
        {
            warnings.push(format!(
                "its description is {length} characters long, more than the {CODEX_DESCRIPTION_LIMIT} characters Codex lists by default for implicit invocation; shorten it with a Destination frontmatter rule or set `{DISABLE_MODEL_INVOCATION}: true`"
            ));
        }
    }
    if text != accepted {
        tree.set_skill_md(text);
    }
    Ok(Adapted { tree, warnings })
}

/// Whether a frontmatter entry is the YAML boolean `true`.
fn is_true(entry: Option<&Entry>) -> bool {
    matches!(
        entry.map(Entry::value),
        Some(Node::Scalar { value, plain: true, tag: None })
            if matches!(value.as_str(), "true" | "True" | "TRUE")
    )
}

/// Length in characters of the `description` of `text`, when it is a scalar.
fn description_length(text: &str) -> Result<Option<usize>, HarnessError> {
    let document = SkillDocument::parse(text)?;
    Ok(
        match document.entry("description").map(|entry| entry.value()) {
            Some(Node::Scalar { value, .. }) => Some(value.chars().count()),
            _ => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(skill_md: &str) -> SkillTree {
        let mut tree = SkillTree::default();
        tree.set_skill_md(skill_md.to_owned());
        tree.insert(
            RelativePath::exact("references/guide.md").unwrap(),
            SkillFile {
                contents: b"Guide.\n".to_vec(),
                executable: false,
            },
        );
        tree
    }

    fn rules(text: &str) -> FrontmatterRules {
        toml::from_str(text).unwrap()
    }

    fn metadata() -> RelativePath {
        RelativePath::exact(CODEX_METADATA_FILE).unwrap()
    }

    const MANUAL: &str =
        "---\nname: commit\ndescription: Commit.\ndisable-model-invocation: true\n---\nBody\n";

    #[test]
    fn every_harness_applies_the_destination_rules() {
        let skill = tree("---\nname: grill\nicon: flame\ndescription: Grill.\n---\nBody\n");
        for harness in [
            Harness::ClaudeCode,
            Harness::Pi,
            Harness::Codex,
            Harness::Agy,
            Harness::Generic,
        ] {
            let adapted = adapt(harness, &rules("remove = ['icon']"), &skill).unwrap();
            assert_eq!(
                adapted.tree.skill_md().unwrap(),
                "---\nname: grill\ndescription: Grill.\n---\nBody\n",
                "{harness:?}"
            );
            assert!(adapted.warnings.is_empty());
            assert!(adapted.tree.get(&metadata()).is_none());
        }
    }

    #[test]
    fn without_rules_the_skill_is_installed_unchanged() {
        // Not even parsed: a frontmatter D1 cannot edit is kept as is.
        let skill = tree("---\ndescription: Use when: asked\n---\nBody\n");
        let adapted = adapt(Harness::ClaudeCode, &FrontmatterRules::default(), &skill).unwrap();
        assert_eq!(adapted.tree, skill);
    }

    #[test]
    fn codex_translates_disable_model_invocation_into_its_metadata_file() {
        let adapted = adapt(Harness::Codex, &FrontmatterRules::default(), &tree(MANUAL)).unwrap();
        let file = adapted.tree.get(&metadata()).unwrap();
        assert_eq!(file.contents, CODEX_MANUAL_ONLY_POLICY.as_bytes());
        assert!(!file.executable);
        assert_eq!(adapted.tree.skill_md().unwrap(), MANUAL);

        // Read before the Destination's rules, which may strip the key.
        let adapted = adapt(
            Harness::Codex,
            &rules("remove = ['disable-model-invocation']"),
            &tree(MANUAL),
        )
        .unwrap();
        assert!(adapted.tree.get(&metadata()).is_some());
        assert_eq!(
            adapted.tree.skill_md().unwrap(),
            "---\nname: commit\ndescription: Commit.\n---\nBody\n"
        );

        // Other harnesses keep the key and get no metadata file.
        let adapted = adapt(
            Harness::ClaudeCode,
            &FrontmatterRules::default(),
            &tree(MANUAL),
        )
        .unwrap();
        assert!(adapted.tree.get(&metadata()).is_none());
    }

    #[test]
    fn codex_translates_only_a_boolean_true() {
        for value in ["false", "\"true\"", "'true'", "yes", "!!str true", "1"] {
            let skill = tree(&format!(
                "---\nname: a\ndescription: A.\ndisable-model-invocation: {value}\n---\n"
            ));
            let adapted = adapt(Harness::Codex, &FrontmatterRules::default(), &skill).unwrap();
            assert!(adapted.tree.get(&metadata()).is_none(), "{value}");
        }
        for value in ["true", "True", "TRUE"] {
            let skill = tree(&format!(
                "---\nname: a\ndisable-model-invocation: {value}\n---\n"
            ));
            let adapted = adapt(Harness::Codex, &FrontmatterRules::default(), &skill).unwrap();
            assert!(adapted.tree.get(&metadata()).is_some(), "{value}");
        }
    }

    #[test]
    fn codex_refuses_to_overwrite_a_shipped_metadata_file() {
        let mut skill = tree(MANUAL);
        skill.insert(
            metadata(),
            SkillFile {
                contents: b"interface: {}\n".to_vec(),
                executable: false,
            },
        );
        assert!(matches!(
            adapt(Harness::Codex, &FrontmatterRules::default(), &skill),
            Err(HarnessError::MetadataFileExists)
        ));
        // Without `disable-model-invocation`, a shipped file is kept.
        let mut skill = tree("---\nname: a\ndescription: A.\n---\n");
        skill.insert(
            metadata(),
            SkillFile {
                contents: b"interface: {}\n".to_vec(),
                executable: false,
            },
        );
        let adapted = adapt(Harness::Codex, &FrontmatterRules::default(), &skill).unwrap();
        assert_eq!(adapted.tree, skill);
    }

    #[test]
    fn codex_warns_about_long_implicit_descriptions() {
        let long = "d".repeat(CODEX_DESCRIPTION_LIMIT + 1);
        let at_limit = "é".repeat(CODEX_DESCRIPTION_LIMIT);
        let implicit = tree(&format!("---\nname: a\ndescription: {long}\n---\n"));
        let adapted = adapt(Harness::Codex, &FrontmatterRules::default(), &implicit).unwrap();
        assert_eq!(adapted.warnings.len(), 1);
        assert!(
            adapted.warnings[0].starts_with("its description is 8001 characters long"),
            "{:?}",
            adapted.warnings
        );

        // Counted in characters, not bytes.
        let ok = tree(&format!("---\nname: a\ndescription: {at_limit}\n---\n"));
        assert!(
            adapt(Harness::Codex, &FrontmatterRules::default(), &ok)
                .unwrap()
                .warnings
                .is_empty()
        );

        // Measured after the Destination's rules.
        let shortened = adapt(
            Harness::Codex,
            &rules("set = { description = 'Short.' }"),
            &implicit,
        )
        .unwrap();
        assert!(shortened.warnings.is_empty());

        // Manual-only Skills are not listed for implicit invocation, and
        // other harnesses have no such limit.
        let manual = tree(&format!(
            "---\nname: a\ndescription: {long}\ndisable-model-invocation: true\n---\n"
        ));
        assert!(
            adapt(Harness::Codex, &FrontmatterRules::default(), &manual)
                .unwrap()
                .warnings
                .is_empty()
        );
        assert!(
            adapt(Harness::Pi, &FrontmatterRules::default(), &implicit)
                .unwrap()
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn codex_reports_an_unreadable_frontmatter() {
        let skill = tree("---\nname: [unclosed\n---\n");
        assert!(matches!(
            adapt(Harness::Codex, &FrontmatterRules::default(), &skill),
            Err(HarnessError::Adaptation(_))
        ));
        assert!(adapt(Harness::Pi, &FrontmatterRules::default(), &skill).is_ok());
    }
}
