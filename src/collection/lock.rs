//! `lock.toml`: tool-managed record, committed in the Collection, of what was
//! accepted for each Skill: pinned Revisions, integrity hashes, provenance,
//! and detected licenses.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::Problem;
use crate::hash::{Digest, is_lower_hex};
use crate::safety::{Name, RelativePath};

/// Lock format written and read by this release.
pub const LOCK_FORMAT: u32 = 1;

/// Parsed `lock.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    /// Lock format; only [`LOCK_FORMAT`] is understood.
    pub format: u32,
    /// Accepted state per Skill, by Skill name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<Name, LockedSkill>,
}

impl Default for Lock {
    fn default() -> Self {
        Self {
            format: LOCK_FORMAT,
            skills: BTreeMap::new(),
        }
    }
}

/// Accepted state of one Skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedSkill {
    /// Hash of the accepted materialised directory `skills/<name>/`.
    pub output_hash: Digest,
    /// Hash of the declaration, D1/D2 rules, and written intent the output
    /// was produced from. Absent for local Skills, which have no inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs_hash: Option<Digest>,
    /// Where the Skill's content comes from: one entry for Copy and Adapt
    /// Skills, one per input for Compositions, none for local Skills.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<Provenance>,
}

/// Origin of an upstream Skill, or of one Composition input, at its pinned
/// Revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Composition input name; absent for Copy and Adapt Skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Name>,
    /// Source name in the Manifest.
    pub source: Name,
    /// Repository URL the Revision was fetched from.
    pub url: String,
    /// Skill directory inside the Source.
    pub upstream_path: RelativePath,
    /// Pinned Revision.
    pub revision: Revision,
    /// License files found for this Skill. Empty when none was detected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub licenses: Vec<LicenseFile>,
}

/// A license file detected in a Source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseFile {
    /// Path of the file relative to the Source root.
    pub path: RelativePath,
    /// Best-effort license identification, such as `MIT`. Absent when the
    /// license could not be identified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// A full Git commit identifier: 40 (SHA-1) or 64 (SHA-256) lowercase
/// hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Revision(String);

impl Revision {
    /// Validates a commit identifier.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRevision`] unless `raw` is 40 or 64 lowercase
    /// hexadecimal digits. Abbreviated identifiers and branch names are not
    /// Revisions.
    pub fn new(raw: &str) -> Result<Self, InvalidRevision> {
        if matches!(raw.len(), 40 | 64) && is_lower_hex(raw) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InvalidRevision(raw.to_owned()))
        }
    }

    /// The commit identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Revision {
    type Error = InvalidRevision;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw)
    }
}

impl From<Revision> for String {
    fn from(revision: Revision) -> Self {
        revision.0
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A value that is not a full commit identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidRevision(String);

impl fmt::Display for InvalidRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a Revision: expected a full commit identifier of 40 or 64 lowercase hexadecimal digits",
            self.0
        )
    }
}

impl std::error::Error for InvalidRevision {}

/// The part of `lock.toml` every format shares: its format number.
#[derive(Deserialize)]
struct LockHeader {
    format: u32,
}

impl Lock {
    /// Format number declared by `lock.toml` text, read without checking the
    /// rest of the file against this release's schema, so a lock written by
    /// a newer release can be recognised as such.
    ///
    /// Returns `None` when the text is not valid TOML or has no integer
    /// `format`; [`Lock::from_toml`] then reports the precise error.
    #[must_use]
    pub fn declared_format(text: &str) -> Option<u32> {
        toml::from_str::<LockHeader>(text)
            .ok()
            .map(|header| header.format)
    }

    /// Parses `lock.toml` text.
    ///
    /// # Errors
    ///
    /// Returns the TOML error when the text is not valid TOML or does not
    /// match the schema.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Serialises the lock as TOML.
    ///
    /// # Errors
    ///
    /// Returns a TOML serialisation error; it cannot occur for a lock built
    /// from parsed text.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Consistency problems that parsing alone cannot catch: a Composition
    /// input recorded by more than one provenance entry.
    #[must_use]
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        for (name, skill) in &self.skills {
            let mut seen = BTreeSet::new();
            for input in skill
                .provenance
                .iter()
                .filter_map(|entry| entry.input.as_ref())
            {
                if !seen.insert(input) {
                    problems.push(Problem::new(
                        format!("skills.{name}.provenance"),
                        format!(
                            "input `{input}` has more than one provenance entry; record each Composition input once"
                        ),
                    ));
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1: &str = "0123456789abcdef0123456789abcdef01234567";

    fn hash(fill: char) -> String {
        format!("sha256:{}", fill.to_string().repeat(64))
    }

    /// A lock covering a Copy, an Adapt, a Composition, and a local Skill.
    fn full() -> String {
        format!(
            r#"format = 1

[skills.grill-me]
output_hash = "{a}"
inputs_hash = "{b}"

[[skills.grill-me.provenance]]
source = "mattpocock"
url = "https://github.com/mattpocock/skills.git"
upstream_path = "skills/grill-me"
revision = "{SHA1}"
licenses = [{{ path = "LICENSE", summary = "MIT" }}]

[skills.grilling]
output_hash = "{c}"
inputs_hash = "{d}"

[[skills.grilling.provenance]]
source = "mattpocock"
url = "https://github.com/mattpocock/skills.git"
upstream_path = "skills/grilling"
revision = "{SHA1}"

[skills.code-review]
output_hash = "{e}"
inputs_hash = "{f}"

[[skills.code-review.provenance]]
input = "review"
source = "mattpocock"
url = "https://github.com/mattpocock/skills.git"
upstream_path = "skills/review"
revision = "{SHA1}"
licenses = [{{ path = "LICENSE", summary = "MIT" }}]

[[skills.code-review.provenance]]
input = "critique"
source = "pstack"
url = "https://github.com/cursor/plugins.git"
upstream_path = "pstack/critique"
revision = "{sha256}"
licenses = [{{ path = "LICENSE" }}, {{ path = "pstack/LICENSE", summary = "Apache-2.0" }}]

[skills.fr-learner-voice]
output_hash = "{a}"
"#,
            a = hash('a'),
            b = hash('b'),
            c = hash('c'),
            d = hash('d'),
            e = hash('e'),
            f = hash('f'),
            sha256 = "f".repeat(64),
        )
    }

    fn name(raw: &str) -> Name {
        Name::new(raw).unwrap()
    }

    #[test]
    fn parses_every_skill_kind() {
        let lock = Lock::from_toml(&full()).unwrap();
        assert_eq!(lock.format, LOCK_FORMAT);
        assert_eq!(lock.skills.len(), 4);

        let copy = &lock.skills[&name("grill-me")];
        assert_eq!(copy.output_hash.as_str(), hash('a'));
        assert_eq!(copy.provenance.len(), 1);
        assert_eq!(copy.provenance[0].input, None);
        assert_eq!(copy.provenance[0].revision.as_str(), SHA1);
        assert_eq!(
            copy.provenance[0].licenses[0].summary.as_deref(),
            Some("MIT")
        );

        let adapt = &lock.skills[&name("grilling")];
        assert!(adapt.provenance[0].licenses.is_empty());

        let composition = &lock.skills[&name("code-review")];
        let inputs = composition
            .provenance
            .iter()
            .map(|entry| entry.input.as_ref().map(Name::as_str))
            .collect::<Vec<_>>();
        assert_eq!(inputs, [Some("review"), Some("critique")]);
        let critique = &composition.provenance[1];
        assert_eq!(critique.revision.as_str().len(), 64);
        assert_eq!(critique.licenses[0].summary, None);
        assert_eq!(critique.licenses[1].path.as_str(), "pstack/LICENSE");

        let local = &lock.skills[&name("fr-learner-voice")];
        assert_eq!(local.inputs_hash, None);
        assert!(local.provenance.is_empty());
    }

    #[test]
    fn round_trips_through_toml() {
        let lock = Lock::from_toml(&full()).unwrap();
        let text = lock.to_toml().unwrap();
        assert_eq!(Lock::from_toml(&text).unwrap(), lock);
        assert_eq!(Lock::from_toml(&text).unwrap().to_toml().unwrap(), text);
    }

    #[test]
    fn default_lock_is_empty_and_current() {
        let text = Lock::default().to_toml().unwrap();
        assert_eq!(text, "format = 1\n");
        assert_eq!(Lock::from_toml(&text).unwrap(), Lock::default());
    }

    #[test]
    fn rejects_malformed_entries() {
        let valid = full();
        let cases = [
            (valid.replace(SHA1, "0123abc"), "is not a Revision"),
            (valid.replace(SHA1, "main"), "is not a Revision"),
            (
                valid.replace(SHA1, &SHA1.to_uppercase()),
                "is not a Revision",
            ),
            (valid.replacen(&hash('a'), "sha256:abc", 1), "sha256:"),
            (
                valid.replacen(
                    "upstream_path = \"skills/grilling\"",
                    "upstream_path = \"../x\"",
                    1,
                ),
                "`..`",
            ),
            (
                valid.replacen("path = \"LICENSE\"", "path = \"/LICENSE\"", 1),
                "must be relative",
            ),
            (
                valid.replacen("format = 1\n", "", 1),
                "missing field `format`",
            ),
            (
                valid.replacen(
                    "[skills.grilling]\n",
                    "[skills.grilling]\nmode = \"adapt\"\n",
                    1,
                ),
                "unknown field `mode`",
            ),
        ];
        for (text, expected) in cases {
            let err = Lock::from_toml(&text).unwrap_err().to_string();
            assert!(err.contains(expected), "expected {expected:?} in:\n{err}");
        }
    }

    #[test]
    fn reads_the_format_of_newer_locks() {
        assert_eq!(Lock::declared_format(&full()), Some(LOCK_FORMAT));
        assert_eq!(
            Lock::declared_format("format = 2\nfoo = 1\n[skills.x]\noutput_hash = 'md5:x'\n"),
            Some(2)
        );
        assert_eq!(Lock::declared_format("foo = 1\n"), None);
        assert_eq!(Lock::declared_format("format = 'one'\n"), None);
        assert_eq!(Lock::declared_format("format = \n"), None);
    }

    #[test]
    fn reports_repeated_composition_inputs() {
        let lock = Lock::from_toml(&full()).unwrap();
        assert_eq!(lock.problems(), []);

        let repeated = full().replacen("input = \"critique\"", "input = \"review\"", 1);
        let problems = Lock::from_toml(&repeated)
            .unwrap()
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            problems,
            [
                "skills.code-review.provenance: input `review` has more than one provenance entry; record each Composition input once"
            ]
        );
    }

    #[test]
    fn revisions_require_full_lowercase_commit_ids() {
        assert!(Revision::new(SHA1).is_ok());
        assert!(Revision::new(&"a".repeat(64)).is_ok());
        for bad in [
            "",
            "HEAD",
            &SHA1[..39],
            &format!("{SHA1}0"),
            &"g".repeat(40),
        ] {
            assert!(Revision::new(bad).is_err(), "{bad:?}");
        }
    }
}
