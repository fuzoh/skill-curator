//! `materialize`: replays local changes to a Skill's deterministic rules or
//! written intent at its pinned Revision, without fetching any Source, and
//! protects manual edits in `skills/<name>/`.
//!
//! For every Skill of the Manifest, `lock.toml` is compared with the
//! Collection:
//!
//! - **Output drift** (the files of `skills/<name>/` no longer hash to the
//!   accepted `output_hash`) means manual edits. They are never overwritten:
//!   the Skill is reported until the user accepts the edits into `lock.toml`
//!   ([`Options::accept`]) or resets the Skill ([`Options::reset`]).
//! - **Input drift** (the declaration, D1/D2 rules, or written intent no
//!   longer hash to `inputs_hash`) is classified against the rules recorded
//!   in the lock entry's `applied` table. D1 changes and newly excluded files
//!   are replayed onto the accepted Skill (`accepted_local`) without an
//!   agent; files newly kept by D2 are added from the pinned Revision unless
//!   the Skill is agent-assisted. Newly kept files and written intent
//!   changes on agent-assisted Skills (Adapt Skills with an intent, and
//!   Compositions) need an agent rewrite and are reported instead, even when
//!   the accepted Skill still equals its deterministic adaptation.
//! - **A changed Source or upstream path** needs `update`; neither a replay
//!   nor a reset rebuilds the Skill from another upstream Skill.
//!
//! The pinned Revision is read from the Source cache only; a Revision the
//! cache lacks is an error, never a fetch. Each Skill is written to a
//! staging directory inside `skills/` and swapped into place, then
//! `lock.toml` is written atomically, one Skill at a time.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::adaptation::files::{FileFilter, FilesError};
use crate::adaptation::frontmatter::{FrontmatterError, apply_rules, replay_rules};
use crate::adaptation::{AdaptationError, Rules, SkillTree, det};
use crate::collection::lock::{AppliedInputs, Lock, LockedSkill, Provenance, Revision};
use crate::collection::manifest::{FrontmatterRules, Skill, Source};
use crate::collection::{
    Collection, CollectionError, Entry, LOCK_FILE, SKILLS_DIR, attribution, inspect,
};
use crate::hash::{Digest, HashError};
use crate::safety::{Name, RelativePath};
use crate::source::{SourceCache, SourceError};

/// Prefix of the staging directories created inside `skills/`.
const STAGING_PREFIX: &str = ".materialize-";

/// The user's explicit choices for Skills with manual edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Skills whose manual edits in `skills/<name>/` are accepted into
    /// `lock.toml`.
    pub accept: Vec<Name>,
    /// Skills reset to their deterministic adaptation at the pinned
    /// Revision, discarding manual edits.
    pub reset: Vec<Name>,
}

/// Failure of `materialize` as a whole, or of one Skill.
#[derive(Debug)]
#[non_exhaustive]
pub enum MaterializeError {
    /// An option names a Skill the Manifest does not declare.
    UnknownSkill {
        /// The name given.
        name: Name,
    },
    /// A Skill is named both to accept and to reset.
    AcceptAndReset {
        /// The Skill.
        name: Name,
    },
    /// `--accept` was given for a local Skill, whose files `materialize`
    /// records on every run.
    AcceptLocal {
        /// The Skill.
        name: Name,
    },
    /// A reset was asked for a Skill without a deterministic adaptation: a
    /// local Skill or a Composition.
    NoDeterministicState {
        /// The Skill.
        name: Name,
        /// Its mode, as written in the Manifest.
        mode: &'static str,
    },
    /// There are no manual edits to accept because `skills/<name>/` is
    /// missing or cannot be hashed.
    NothingToAccept {
        /// The Skill.
        name: Name,
        /// Why.
        reason: String,
        /// Whether `--reset` can rebuild the Skill (a Copy or Adapt Skill).
        resettable: bool,
    },
    /// `lock.toml` does not record exactly one provenance entry for a Copy
    /// or Adapt Skill.
    Provenance {
        /// The Skill.
        name: Name,
        /// Number of entries found.
        entries: usize,
    },
    /// `skills/` is a symbolic link or not a directory.
    SkillsDir {
        /// Path of `skills/`.
        path: PathBuf,
    },
    /// A Skill directory could not be replaced.
    Write {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// A Skill directory could not be replaced, nor its previous files put
    /// back; they are kept at `kept`.
    Restore {
        /// Path of the Skill directory.
        path: PathBuf,
        /// Where the previous files of the Skill are kept.
        kept: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The Collection could not be read or written.
    Collection(CollectionError),
    /// The pinned Revision could not be read from the Source cache.
    Source(SourceError),
    /// The deterministic rules could not be applied.
    Adaptation(AdaptationError),
}

impl fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSkill { name } => {
                write!(f, "Skill `{name}` is not declared in manifest.toml")
            }
            Self::AcceptAndReset { name } => write!(
                f,
                "Skill `{name}` is named with both `--accept` and `--reset`; choose one"
            ),
            Self::AcceptLocal { name } => write!(
                f,
                "Skill `{name}` is a local Skill: `materialize` records its files in {LOCK_FILE} on every run, so there is nothing to accept; drop `--accept {name}`"
            ),
            Self::NoDeterministicState { name, mode } => write!(
                f,
                "Skill `{name}` cannot be reset: a {mode} Skill has no deterministic adaptation; restore `{SKILLS_DIR}/{name}/` from the Collection's Git history instead"
            ),
            Self::NothingToAccept {
                name,
                reason,
                resettable: true,
            } => write!(
                f,
                "Skill `{name}` has no manual edits to accept: {reason}; use `--reset {name}` to rebuild it"
            ),
            Self::NothingToAccept {
                name,
                reason,
                resettable: false,
            } => write!(
                f,
                "Skill `{name}` has no manual edits to accept: {reason}; restore `{SKILLS_DIR}/{name}/` from the Collection's Git history"
            ),
            Self::Provenance { name, entries } => write!(
                f,
                "{LOCK_FILE} records {entries} provenance entries for Skill `{name}`, expected one; run `skill-curator update` to record it again"
            ),
            Self::SkillsDir { path } => write!(
                f,
                "{} must be a directory of the Collection, not a symbolic link or another kind of file",
                path.display()
            ),
            Self::Write { path, source } => write!(f, "cannot write {}: {source}", path.display()),
            Self::Restore { path, kept, source } => write!(
                f,
                "cannot write {}: {source}; its previous files could not be put back and are kept in {}",
                path.display(),
                kept.display()
            ),
            Self::Collection(err) => err.fmt(f),
            Self::Source(err) => err.fmt(f),
            Self::Adaptation(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for MaterializeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Write { source, .. } | Self::Restore { source, .. } => Some(source),
            Self::Collection(err) => Some(err),
            Self::Source(err) => Some(err),
            Self::Adaptation(err) => Some(err),
            _ => None,
        }
    }
}

impl From<CollectionError> for MaterializeError {
    fn from(err: CollectionError) -> Self {
        Self::Collection(err)
    }
}

impl From<SourceError> for MaterializeError {
    fn from(err: SourceError) -> Self {
        Self::Source(err)
    }
}

impl From<AdaptationError> for MaterializeError {
    fn from(err: AdaptationError) -> Self {
        Self::Adaptation(err)
    }
}

impl From<FilesError> for MaterializeError {
    fn from(err: FilesError) -> Self {
        Self::Adaptation(err.into())
    }
}

impl From<FrontmatterError> for MaterializeError {
    fn from(err: FrontmatterError) -> Self {
        Self::Adaptation(err.into())
    }
}

/// What a replay changed in an accepted Skill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Whether `SKILL.md`'s frontmatter changed.
    pub frontmatter: bool,
    /// Files removed because the D2 rules now exclude them.
    pub removed: Vec<RelativePath>,
    /// Files added from the pinned Revision because the D2 rules now keep
    /// them.
    pub added: Vec<RelativePath>,
    /// Whether the Skill was rebuilt as `det(R)` because it was still the
    /// deterministic adaptation of its pinned Revision.
    pub rebuilt: bool,
}

impl Changes {
    fn is_empty(&self) -> bool {
        !self.frontmatter && self.removed.is_empty() && self.added.is_empty() && !self.rebuilt
    }
}

/// What happened to one Skill.
#[derive(Debug)]
#[non_exhaustive]
pub enum Outcome {
    /// Nothing changed since the accepted state.
    Unchanged,
    /// A local Skill's files were recorded in `lock.toml`.
    RecordedLocal,
    /// Rule changes were replayed at the pinned Revision. Empty changes
    /// mean the inputs changed without changing the Skill; only
    /// `lock.toml` was updated.
    Replayed {
        /// What changed in `skills/<name>/`.
        changes: Changes,
    },
    /// Manual edits were accepted into `lock.toml`.
    Accepted {
        /// The Skill's written intent, which should now describe the edits.
        intent: Option<RelativePath>,
        /// Whether the edited `SKILL.md` breaks the Skill's D1 rules, or has
        /// a frontmatter they cannot be applied to.
        breaks_frontmatter_rules: bool,
    },
    /// `--accept` was given but the Skill has no manual edits.
    NothingToAccept,
    /// The Skill was reset to its deterministic adaptation at the pinned
    /// Revision.
    Reset {
        /// The pinned Revision.
        revision: Revision,
        /// The written intent, which is no longer applied.
        intent: Option<RelativePath>,
    },
    /// The Skill has no accepted state yet.
    NotMaterialized,
    /// `skills/<name>/` has manual edits that need an explicit choice.
    ManualEdits {
        /// What differs.
        reason: String,
        /// Whether `--reset` can discard them (a Copy or Adapt Skill).
        resettable: bool,
    },
    /// The change needs an agent rewrite, or a reset when the Skill has a
    /// deterministic adaptation.
    NeedsAgent {
        /// What changed.
        reason: String,
        /// Whether `--reset` can rebuild the Skill (an Adapt Skill).
        resettable: bool,
    },
    /// The declared upstream Skill differs from the pinned one.
    NeedsUpdate {
        /// What differs.
        reason: String,
    },
    /// The change cannot be replayed safely; a reset is needed, or `update`
    /// for a Skill without a deterministic adaptation.
    Blocked {
        /// Why.
        reason: String,
        /// Whether `--reset` can rebuild the Skill (a Copy or Adapt Skill).
        resettable: bool,
    },
    /// The Skill could not be processed.
    Failed(MaterializeError),
}

impl Outcome {
    /// Whether the Skill still needs the user's attention.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(
            self,
            Self::NotMaterialized
                | Self::ManualEdits { .. }
                | Self::NeedsAgent { .. }
                | Self::NeedsUpdate { .. }
                | Self::Blocked { .. }
                | Self::Failed(_)
        )
    }

    /// One-line description for Skill `name`, with the command to run next
    /// when the Skill needs attention.
    #[must_use]
    pub fn describe(&self, name: &Name) -> String {
        let dir = format!("{SKILLS_DIR}/{name}/");
        match self {
            Self::Unchanged => format!("{name}: unchanged"),
            Self::RecordedLocal => format!("{name}: recorded the local Skill in {LOCK_FILE}"),
            Self::Replayed { changes } if changes.is_empty() => format!(
                "{name}: the inputs changed without changing the Skill; updated {LOCK_FILE}"
            ),
            Self::Replayed { changes } => {
                let mut parts = Vec::new();
                if changes.rebuilt {
                    parts.push("rebuilt its deterministic adaptation".to_owned());
                }
                if changes.frontmatter && !changes.rebuilt {
                    parts.push("replayed the frontmatter rules (D1)".to_owned());
                }
                if !changes.removed.is_empty() {
                    parts.push(format!("removed {}", list(&changes.removed)));
                }
                if !changes.added.is_empty() {
                    parts.push(format!("added {}", list(&changes.added)));
                }
                format!(
                    "{name}: {} at the pinned Revision; updated {LOCK_FILE}",
                    parts.join(", ")
                )
            }
            Self::Accepted {
                intent,
                breaks_frontmatter_rules,
            } => {
                let mut text =
                    format!("{name}: accepted the manual edits in {dir} into {LOCK_FILE}");
                if let Some(intent) = intent {
                    text.push_str(&format!(
                        "; update the written intent `{intent}` so the next agent rewrite keeps these edits"
                    ));
                }
                if *breaks_frontmatter_rules {
                    text.push_str(
                        "; warning: SKILL.md no longer satisfies the Skill's frontmatter rules (D1), which the next rewrite enforces again",
                    );
                }
                text
            }
            Self::NothingToAccept => format!("{name}: no manual edits to accept"),
            Self::Reset { revision, intent } => {
                let mut text = format!(
                    "{name}: reset {dir} to its deterministic adaptation at Revision {revision}"
                );
                if let Some(intent) = intent {
                    text.push_str(&format!(
                        "; the written intent `{intent}` is no longer applied and needs an agent rewrite"
                    ));
                }
                text
            }
            Self::NotMaterialized => format!(
                "{name}: not materialised yet; run `skill-curator update` to fetch and accept it"
            ),
            Self::ManualEdits {
                reason,
                resettable: true,
            } => format!(
                "{name}: manual edits in {dir} ({reason}); run `skill-curator materialize --accept {name}` to keep them or `skill-curator materialize --reset {name}` to discard them"
            ),
            Self::ManualEdits {
                reason,
                resettable: false,
            } => format!(
                "{name}: manual edits in {dir} ({reason}); run `skill-curator materialize --accept {name}` to keep them, or restore {dir} from the Collection's Git history to discard them"
            ),
            Self::NeedsAgent {
                reason,
                resettable: true,
            } => format!(
                "{name}: needs an agent rewrite: {reason}; `materialize` does not run agents; `skill-curator materialize --reset {name}` resets it to its deterministic adaptation instead"
            ),
            Self::NeedsAgent {
                reason,
                resettable: false,
            } => format!(
                "{name}: needs an agent rewrite: {reason}; `materialize` does not run agents, and a composition Skill has no deterministic adaptation to reset to"
            ),
            Self::NeedsUpdate { reason } => {
                format!("{name}: {reason}; run `skill-curator update`")
            }
            Self::Blocked {
                reason,
                resettable: true,
            } => format!(
                "{name}: cannot replay the change: {reason}; run `skill-curator materialize --reset {name}` to rebuild it"
            ),
            Self::Blocked {
                reason,
                resettable: false,
            } => format!(
                "{name}: cannot replay the change: {reason}; a composition Skill has no deterministic adaptation to reset to; run `skill-curator update` to accept it again"
            ),
            Self::Failed(err) => format!("{name}: failed: {err}"),
        }
    }
}

fn list(paths: &[RelativePath]) -> String {
    paths
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The outcome of every Skill, in Manifest order. A Skill whose manual
/// edits were accepted appears twice: the acceptance, then the replay.
#[derive(Debug, Default)]
pub struct Report {
    /// Outcomes, by Skill.
    pub skills: Vec<(Name, Outcome)>,
}

impl Report {
    /// Number of outcomes that still need the user's attention.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.skills
            .iter()
            .filter(|(_, outcome)| outcome.is_pending())
            .count()
    }
}

/// Runs `materialize` on `collection`, reading pinned Revisions from
/// `cache` without fetching.
///
/// # Errors
///
/// Returns a [`MaterializeError`] when `options` name an undeclared Skill,
/// the same Skill twice, or a Skill that cannot be reset. Failures of single
/// Skills are reported as [`Outcome::Failed`] instead, after the other
/// Skills are processed.
pub fn materialize(
    collection: &Collection,
    cache: &SourceCache,
    options: &Options,
) -> Result<Report, MaterializeError> {
    check_options(collection, options)?;
    let mut run = Run {
        collection,
        cache,
        lock: collection.lock().clone(),
    };
    let mut report = Report::default();
    for (name, skill) in &collection.manifest().skills {
        if matches!(skill, Skill::Local(_)) {
            report.skills.push((name.clone(), outcome(run.local(name))));
            continue;
        }
        if options.accept.contains(name) {
            let accepted = outcome(run.accept(name, skill));
            let failed = matches!(accepted, Outcome::Failed(_));
            report.skills.push((name.clone(), accepted));
            if failed {
                continue;
            }
        }
        let result = if options.reset.contains(name) {
            run.reset(name, skill)
        } else {
            run.replay(name, skill)
        };
        report.skills.push((name.clone(), outcome(result)));
    }
    Ok(report)
}

fn outcome(result: Result<Outcome, MaterializeError>) -> Outcome {
    result.unwrap_or_else(Outcome::Failed)
}

fn check_options(collection: &Collection, options: &Options) -> Result<(), MaterializeError> {
    let skills = &collection.manifest().skills;
    for name in options.accept.iter().chain(&options.reset) {
        if !skills.contains_key(name) {
            return Err(MaterializeError::UnknownSkill { name: name.clone() });
        }
    }
    for name in &options.accept {
        if let Some(Skill::Local(_)) = skills.get(name) {
            return Err(MaterializeError::AcceptLocal { name: name.clone() });
        }
    }
    for name in &options.reset {
        if options.accept.contains(name) {
            return Err(MaterializeError::AcceptAndReset { name: name.clone() });
        }
        if let Some(skill) = skills.get(name).filter(|skill| !resettable(skill)) {
            return Err(MaterializeError::NoDeterministicState {
                name: name.clone(),
                mode: skill.mode(),
            });
        }
    }
    Ok(())
}

/// State of `skills/<name>/` against its accepted output hash.
enum OutputState {
    InSync,
    Changed(Digest),
    Missing,
    Unhashable(String),
}

struct Run<'a> {
    collection: &'a Collection,
    cache: &'a SourceCache,
    lock: Lock,
}

impl Run<'_> {
    fn output_state(
        &self,
        name: &Name,
        accepted: &Digest,
    ) -> Result<OutputState, MaterializeError> {
        match self.collection.output_digest(name) {
            Ok(digest) if digest == *accepted => Ok(OutputState::InSync),
            Ok(digest) => Ok(OutputState::Changed(digest)),
            Err(CollectionError::Hash(HashError::Io { source, .. }))
                if source.kind() == io::ErrorKind::NotFound =>
            {
                Ok(OutputState::Missing)
            }
            Err(CollectionError::Hash(
                err @ (HashError::Symlink { .. }
                | HashError::SpecialFile { .. }
                | HashError::NotADirectory { .. }
                | HashError::NonUtf8Name { .. }),
            )) => Ok(OutputState::Unhashable(err.to_string())),
            Err(err) => Err(err.into()),
        }
    }

    fn write_lock(&self) -> Result<(), MaterializeError> {
        Ok(self.lock.write(self.collection.root())?)
    }

    fn local(&mut self, name: &Name) -> Result<Outcome, MaterializeError> {
        let output_hash = self.collection.output_digest(name)?;
        let previous = self.lock.skills.get(name);
        if previous.is_some_and(|locked| {
            locked.output_hash == output_hash
                && locked.inputs_hash.is_none()
                && locked.provenance.is_empty()
        }) {
            return Ok(Outcome::Unchanged);
        }
        let had_provenance = previous.is_some_and(|locked| !locked.provenance.is_empty());
        self.lock.skills.insert(
            name.clone(),
            LockedSkill {
                output_hash,
                inputs_hash: None,
                provenance: Vec::new(),
                applied: None,
            },
        );
        self.write_lock()?;
        if had_provenance {
            attribution::write(self.collection.root(), &self.lock)?;
        }
        Ok(Outcome::RecordedLocal)
    }

    fn accept(&mut self, name: &Name, skill: &Skill) -> Result<Outcome, MaterializeError> {
        let resettable = resettable(skill);
        let Some(locked) = self.lock.skills.get(name) else {
            return Ok(Outcome::NotMaterialized);
        };
        let reason = match self.output_state(name, &locked.output_hash)? {
            OutputState::InSync => return Ok(Outcome::NothingToAccept),
            OutputState::Changed(digest) => {
                return self.accept_edits(name, skill, digest);
            }
            OutputState::Missing => format!("`{SKILLS_DIR}/{name}/` is missing"),
            OutputState::Unhashable(reason) => reason,
        };
        Err(MaterializeError::NothingToAccept {
            name: name.clone(),
            reason,
            resettable,
        })
    }

    fn accept_edits(
        &mut self,
        name: &Name,
        skill: &Skill,
        digest: Digest,
    ) -> Result<Outcome, MaterializeError> {
        let tree = SkillTree::read(&self.collection.skill_dir(name))?;
        let text = tree.skill_md()?;
        // The edits are the user's to keep: a `SKILL.md` the D1 rules would
        // change, or whose frontmatter they cannot be applied to, is accepted
        // with a warning. Without D1 rules its frontmatter is not parsed.
        let rules = Rules::of(self.collection.manifest(), skill);
        let breaks_frontmatter_rules =
            !apply_rules(text, &rules.frontmatter).is_ok_and(|applied| applied == text);
        if let Some(locked) = self.lock.skills.get_mut(name) {
            locked.output_hash = digest;
        }
        self.write_lock()?;
        Ok(Outcome::Accepted {
            intent: skill.intent().cloned(),
            breaks_frontmatter_rules,
        })
    }

    fn reset(&mut self, name: &Name, skill: &Skill) -> Result<Outcome, MaterializeError> {
        let Some(locked) = self.lock.skills.get(name) else {
            return Ok(Outcome::NotMaterialized);
        };
        // The pinned Revision holds the old upstream Skill; rebuilding it and
        // recording the current declaration would hide the change for good.
        if let Some(reason) = identity_change(skill, &locked.provenance) {
            return Ok(Outcome::NeedsUpdate { reason });
        }
        let provenance = single_provenance(name, locked)?.clone();
        let upstream = self.upstream(&provenance)?;
        let tree = det(&upstream, &Rules::of(self.collection.manifest(), skill))?;
        self.replace_skill(name, &tree)?;
        let intent = skill.intent().cloned();
        let inputs_hash = self.collection.inputs_digest_without_intent(name)?;
        let mut applied = self.collection.applied_inputs(name)?.unwrap_or_default();
        applied.intent_hash = None;
        let output_hash = self.collection.output_digest(name)?;
        if let Some(locked) = self.lock.skills.get_mut(name) {
            locked.output_hash = output_hash;
            locked.inputs_hash = inputs_hash;
            locked.applied = Some(applied);
        }
        self.write_lock()?;
        Ok(Outcome::Reset {
            revision: provenance.revision,
            intent,
        })
    }

    fn replay(&mut self, name: &Name, skill: &Skill) -> Result<Outcome, MaterializeError> {
        let Some(locked) = self.lock.skills.get(name).cloned() else {
            return Ok(Outcome::NotMaterialized);
        };
        let resettable = resettable(skill);
        let reason = match self.output_state(name, &locked.output_hash)? {
            OutputState::InSync => None,
            OutputState::Changed(_) => Some(format!(
                "its files differ from the accepted output recorded in {LOCK_FILE}"
            )),
            OutputState::Missing => Some("the directory is missing".to_owned()),
            OutputState::Unhashable(reason) => Some(reason),
        };
        if let Some(reason) = reason {
            return Ok(Outcome::ManualEdits { reason, resettable });
        }
        let inputs_hash = self.collection.inputs_digest(name)?;
        if inputs_hash == locked.inputs_hash {
            return Ok(Outcome::Unchanged);
        }
        if let Some(reason) = identity_change(skill, &locked.provenance) {
            return Ok(Outcome::NeedsUpdate { reason });
        }
        let current = self.collection.applied_inputs(name)?.unwrap_or_default();
        let Some(old) = &locked.applied else {
            return Ok(Outcome::Blocked {
                reason: format!(
                    "{LOCK_FILE} does not record which rules produced the accepted Skill"
                ),
                resettable,
            });
        };
        let agent = matches!(skill, Skill::Composition(_)) || skill.intent().is_some();
        if old.intent_hash != current.intent_hash {
            return Ok(match (&old.intent_hash, agent) {
                (None, true) => Outcome::NeedsAgent {
                    reason: "the written intent has not been applied yet".to_owned(),
                    resettable,
                },
                (Some(_), true) => Outcome::NeedsAgent {
                    reason: "the written intent changed".to_owned(),
                    resettable,
                },
                (_, false) => Outcome::Blocked {
                    reason: "the written intent was removed, but the accepted Skill still holds the agent rewrite it produced".to_owned(),
                    resettable,
                },
            });
        }
        if old.inputs != current.inputs {
            return Ok(Outcome::NeedsAgent {
                reason: "the file rules (D2) of a Composition input changed".to_owned(),
                resettable,
            });
        }

        let mut accepted = SkillTree::read(&self.collection.skill_dir(name))?;
        let changes = match skill {
            Skill::Composition(_) => replay_composition(&mut accepted, old, &current)?,
            _ => {
                let provenance = single_provenance(name, &locked)?;
                match self.replay_upstream(&mut accepted, provenance, old, &current, agent)? {
                    Ok(changes) => changes,
                    Err(outcome) => return Ok(outcome),
                }
            }
        };
        if !changes.is_empty() {
            self.replace_skill(name, &accepted)?;
        }
        let output_hash = self.collection.output_digest(name)?;
        if let Some(entry) = self.lock.skills.get_mut(name) {
            entry.output_hash = output_hash;
            entry.inputs_hash = inputs_hash;
            entry.applied = Some(AppliedInputs {
                intent_hash: old.intent_hash.clone(),
                ..current
            });
        }
        self.write_lock()?;
        Ok(Outcome::Replayed { changes })
    }

    /// Replays D2 and D1 changes of a Copy or Adapt Skill onto `accepted`,
    /// reading the pinned Revision. Returns the outcome to report instead
    /// when the change cannot be replayed.
    fn replay_upstream(
        &self,
        accepted: &mut SkillTree,
        provenance: &Provenance,
        old: &AppliedInputs,
        current: &AppliedInputs,
        agent: bool,
    ) -> Result<Result<Changes, Outcome>, MaterializeError> {
        let mut changes = Changes::default();
        if old.files == current.files && old.frontmatter == current.frontmatter {
            return Ok(Ok(changes));
        }
        let upstream = self.upstream(provenance)?;
        let old_rules = rules_of(old);
        let new_rules = rules_of(current);
        let old_kept = FileFilter::new(&old_rules.files)?.select(upstream.paths())?;
        let new_kept = FileFilter::new(&new_rules.files)?.select(upstream.paths())?;
        let added = new_kept.difference(&old_kept).cloned().collect::<Vec<_>>();
        // Checked before the rebuild below: an agent-assisted Skill whose
        // rewrite left it equal to `det(R)` still never saw the new files,
        // and its recorded intent would claim otherwise.
        if agent && !added.is_empty() {
            return Ok(Err(Outcome::NeedsAgent {
                reason: format!(
                    "the file rules (D2) now keep files the agent rewrite has not seen: {}",
                    list(&added)
                ),
                resettable: true,
            }));
        }
        // Still exactly the deterministic adaptation: rebuild it, so the
        // result is the same as a fresh `det(R)`.
        if det(&upstream, &old_rules)? == *accepted {
            let rebuilt = det(&upstream, &new_rules)?;
            changes.rebuilt = rebuilt != *accepted;
            *accepted = rebuilt;
            return Ok(Ok(changes));
        }
        for path in added {
            if accepted.get(&path).is_some() {
                return Ok(Err(Outcome::Blocked {
                    reason: format!(
                        "the file rules (D2) now keep `{path}`, but the accepted Skill already has a file there"
                    ),
                    resettable: true,
                }));
            }
            if let Some(file) = upstream.get(&path) {
                accepted.insert(path.clone(), file.clone());
                changes.added.push(path);
            }
        }
        for path in old_kept.difference(&new_kept) {
            if accepted.remove(path).is_some() {
                changes.removed.push(path.clone());
            }
        }
        if old.frontmatter != current.frontmatter {
            changes.frontmatter = replay_frontmatter(
                accepted,
                Some(upstream.skill_md()?),
                &old_rules.frontmatter,
                &new_rules.frontmatter,
            )?;
        }
        Ok(Ok(changes))
    }

    /// The upstream Skill at its pinned Revision, read from the cache only.
    fn upstream(&self, provenance: &Provenance) -> Result<SkillTree, MaterializeError> {
        let source = Source {
            url: provenance.url.clone(),
            branch: None,
        };
        let extracted = self.cache.extract_cached(
            &provenance.source,
            &source,
            &provenance.revision,
            &provenance.upstream_path,
        )?;
        Ok(SkillTree::read(extracted.path())?)
    }

    /// Replaces `skills/<name>/` with `tree`: writes it into a staging
    /// directory inside `skills/`, then swaps it into place, so a failure
    /// never leaves a half-written Skill.
    fn replace_skill(&self, name: &Name, tree: &SkillTree) -> Result<(), MaterializeError> {
        let root = self.collection.root();
        let skills = root.join(SKILLS_DIR);
        let write_error = |path: &Path| {
            let path = path.to_path_buf();
            move |source| MaterializeError::Write { path, source }
        };
        match inspect(root, &[SKILLS_DIR])? {
            Entry::Dir => {}
            Entry::Missing => fs::create_dir(&skills).map_err(write_error(&skills))?,
            Entry::Symlink(_) | Entry::File | Entry::Other => {
                return Err(MaterializeError::SkillsDir { path: skills });
            }
        }
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&skills)
            .map_err(write_error(&skills))?;
        let new = staging.path().join("new");
        tree.write_new(&new)?;
        let target = self.collection.skill_dir(name);
        let old = staging.path().join("old");
        let had_old = match fs::symlink_metadata(&target) {
            Ok(_) => {
                fs::rename(&target, &old).map_err(write_error(&target))?;
                true
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => false,
            Err(err) => return Err(write_error(&target)(err)),
        };
        if let Err(err) = fs::rename(&new, &target) {
            // Put the previous Skill back; the swap failed as a whole. If
            // even that fails, keep the staging directory holding it rather
            // than deleting the only copy.
            if had_old && fs::rename(&old, &target).is_err() {
                return Err(MaterializeError::Restore {
                    path: target,
                    kept: staging.keep().join("old"),
                    source: err,
                });
            }
            return Err(write_error(&target)(err));
        }
        drop(staging);
        Ok(())
    }
}

/// Replays a D1 change onto an accepted Composition; it has no single
/// upstream to restore keys from.
fn replay_composition(
    accepted: &mut SkillTree,
    old: &AppliedInputs,
    current: &AppliedInputs,
) -> Result<Changes, MaterializeError> {
    let frontmatter = old.frontmatter != current.frontmatter
        && replay_frontmatter(
            accepted,
            None,
            &rules_of(old).frontmatter,
            &rules_of(current).frontmatter,
        )?;
    Ok(Changes {
        frontmatter,
        ..Changes::default()
    })
}

/// Replays a change of D1 rules onto the accepted `SKILL.md`, restoring keys
/// from the `upstream` `SKILL.md` when given. Returns whether it changed.
fn replay_frontmatter(
    accepted: &mut SkillTree,
    upstream: Option<&str>,
    old: &[&FrontmatterRules],
    new: &[&FrontmatterRules],
) -> Result<bool, MaterializeError> {
    let before = accepted.skill_md()?;
    let after = replay_rules(before, upstream, old, new)?;
    let changed = after != before;
    if changed {
        accepted.set_skill_md(after);
    }
    Ok(changed)
}

/// Whether `--reset` can rebuild `skill`: only Copy and Adapt Skills have a
/// deterministic adaptation.
fn resettable(skill: &Skill) -> bool {
    matches!(skill, Skill::Copy(_) | Skill::Adapt(_))
}

fn rules_of(applied: &AppliedInputs) -> Rules<'_> {
    Rules {
        files: applied.files.iter().collect(),
        frontmatter: applied.frontmatter.iter().collect(),
    }
}

fn single_provenance<'a>(
    name: &Name,
    locked: &'a LockedSkill,
) -> Result<&'a Provenance, MaterializeError> {
    match locked.provenance.as_slice() {
        [provenance] if provenance.input.is_none() => Ok(provenance),
        entries => Err(MaterializeError::Provenance {
            name: name.clone(),
            entries: entries.len(),
        }),
    }
}

/// How the declared upstream Skills differ from the pinned ones, if they do.
fn identity_change(skill: &Skill, provenance: &[Provenance]) -> Option<String> {
    let declared = match skill {
        Skill::Copy(copy) => vec![(None, &copy.source, &copy.upstream_path)],
        Skill::Adapt(adapt) => vec![(None, &adapt.source, &adapt.upstream_path)],
        Skill::Composition(composition) => composition
            .inputs
            .iter()
            .map(|(input, declared)| (Some(input), &declared.source, &declared.upstream_path))
            .collect(),
        Skill::Local(_) => return None,
    };
    let declared = declared.into_iter().collect::<BTreeSet<_>>();
    let pinned = provenance
        .iter()
        .map(|entry| (entry.input.as_ref(), &entry.source, &entry.upstream_path))
        .collect::<BTreeSet<_>>();
    (declared != pinned).then(|| {
        "the declared upstream Skill (Source or upstream path) differs from the one pinned in lock.toml"
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::manifest::{AdaptSkill, CopySkill, FileRules, FrontmatterRules};

    fn name(raw: &str) -> Name {
        Name::new(raw).unwrap()
    }

    fn path(raw: &str) -> RelativePath {
        RelativePath::new(raw).unwrap()
    }

    fn provenance(input: Option<&str>, upstream_path: &str) -> Provenance {
        Provenance {
            input: input.map(name),
            source: name("upstream"),
            url: "file:///upstream.git".to_owned(),
            upstream_path: path(upstream_path),
            revision: Revision::new(&"a".repeat(40)).unwrap(),
            licenses: Vec::new(),
        }
    }

    fn copy(source: &str, upstream_path: &str) -> Skill {
        Skill::Copy(CopySkill {
            source: name(source),
            upstream_path: path(upstream_path),
        })
    }

    #[test]
    fn identity_changes_compare_source_and_upstream_path() {
        let pinned = [provenance(None, "skills/a")];
        assert_eq!(
            identity_change(&copy("upstream", "skills/a"), &pinned),
            None
        );
        assert!(identity_change(&copy("upstream", "skills/b"), &pinned).is_some());
        assert!(identity_change(&copy("other", "skills/a"), &pinned).is_some());
        assert!(identity_change(&copy("upstream", "skills/a"), &[]).is_some());
        let adapt = Skill::Adapt(AdaptSkill {
            source: name("upstream"),
            upstream_path: path("skills/a"),
            presets: Vec::new(),
            frontmatter: FrontmatterRules::default(),
            files: FileRules::default(),
            intent: Some(path("intents/a.md")),
        });
        assert_eq!(identity_change(&adapt, &pinned), None);
    }

    #[test]
    fn copy_and_adapt_skills_need_exactly_one_provenance_entry() {
        let locked = |provenance| LockedSkill {
            output_hash: crate::hash::intent_digest(b""),
            inputs_hash: None,
            provenance,
            applied: None,
        };
        let one = locked(vec![provenance(None, "skills/a")]);
        assert_eq!(
            single_provenance(&name("a"), &one).unwrap().upstream_path,
            path("skills/a")
        );
        for entries in [
            vec![],
            vec![provenance(None, "skills/a"), provenance(None, "skills/b")],
            vec![provenance(Some("input"), "skills/a")],
        ] {
            assert!(matches!(
                single_provenance(&name("a"), &locked(entries)),
                Err(MaterializeError::Provenance { .. })
            ));
        }
    }

    #[test]
    fn only_unresolved_outcomes_are_pending() {
        let reason = || "why".to_owned();
        for outcome in [
            Outcome::NotMaterialized,
            Outcome::ManualEdits {
                reason: reason(),
                resettable: true,
            },
            Outcome::NeedsAgent {
                reason: reason(),
                resettable: false,
            },
            Outcome::NeedsUpdate { reason: reason() },
            Outcome::Blocked {
                reason: reason(),
                resettable: true,
            },
            Outcome::Failed(MaterializeError::UnknownSkill { name: name("a") }),
        ] {
            assert!(outcome.is_pending(), "{outcome:?}");
        }
        for outcome in [
            Outcome::Unchanged,
            Outcome::RecordedLocal,
            Outcome::NothingToAccept,
            Outcome::Replayed {
                changes: Changes::default(),
            },
            Outcome::Accepted {
                intent: None,
                breaks_frontmatter_rules: false,
            },
        ] {
            assert!(!outcome.is_pending(), "{outcome:?}");
        }
        let report = Report {
            skills: vec![
                (name("a"), Outcome::Unchanged),
                (name("b"), Outcome::NotMaterialized),
                (
                    name("c"),
                    Outcome::Blocked {
                        reason: reason(),
                        resettable: false,
                    },
                ),
            ],
        };
        assert_eq!(report.pending(), 2);
    }

    #[test]
    fn only_skills_with_a_deterministic_adaptation_are_told_to_reset() {
        let s = name("s");
        let hints = |resettable: bool| {
            let reason = || "why".to_owned();
            [
                Outcome::ManualEdits {
                    reason: reason(),
                    resettable,
                },
                Outcome::NeedsAgent {
                    reason: reason(),
                    resettable,
                },
                Outcome::Blocked {
                    reason: reason(),
                    resettable,
                },
            ]
            .map(|outcome| outcome.describe(&s))
        };
        for hint in hints(true) {
            assert!(
                hint.contains("`skill-curator materialize --reset s`"),
                "{hint}"
            );
        }
        assert_eq!(
            hints(false),
            [
                "s: manual edits in skills/s/ (why); run `skill-curator materialize --accept s` to keep them, or restore skills/s/ from the Collection's Git history to discard them",
                "s: needs an agent rewrite: why; `materialize` does not run agents, and a composition Skill has no deterministic adaptation to reset to",
                "s: cannot replay the change: why; a composition Skill has no deterministic adaptation to reset to; run `skill-curator update` to accept it again",
            ]
        );
        let nothing = |resettable| {
            MaterializeError::NothingToAccept {
                name: s.clone(),
                reason: "why".to_owned(),
                resettable,
            }
            .to_string()
        };
        assert!(nothing(true).ends_with("use `--reset s` to rebuild it"));
        assert!(nothing(false).ends_with("restore `skills/s/` from the Collection's Git history"));
        assert!(resettable(&copy("upstream", "skills/a")));
    }

    #[test]
    fn replay_descriptions_list_every_change() {
        let changes = Changes {
            frontmatter: true,
            removed: vec![path("evals/a.md")],
            added: vec![path("refs/b.md"), path("refs/c.md")],
            rebuilt: false,
        };
        assert_eq!(
            Outcome::Replayed { changes }.describe(&name("s")),
            "s: replayed the frontmatter rules (D1), removed `evals/a.md`, added `refs/b.md`, `refs/c.md` at the pinned Revision; updated lock.toml"
        );
        assert_eq!(
            Outcome::Replayed {
                changes: Changes::default()
            }
            .describe(&name("s")),
            "s: the inputs changed without changing the Skill; updated lock.toml"
        );
        let accepted = Outcome::Accepted {
            intent: Some(path("intents/s.md")),
            breaks_frontmatter_rules: true,
        }
        .describe(&name("s"));
        assert!(accepted.contains("update the written intent `intents/s.md`"));
        assert!(accepted.contains("no longer satisfies the Skill's frontmatter rules (D1)"));
    }
}
