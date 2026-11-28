//! `install`: writes self-contained copies of the accepted Skills, and the
//! global instruction files assembled from Fragments, into every enabled
//! Destination, and tracks what it wrote so it never touches anything else.
//!
//! Each accepted Skill (`skills/<name>/`, matching its `output_hash` in
//! `lock.toml`) is adapted for each enabled Destination ([`harness`]) and
//! written as regular files, never symbolic links, below the Destination's
//! Skills directory. Each Destination with `instructions` gets its global
//! instruction file assembled from its Fragments ([`fragments`]).
//!
//! The ownership [`state`] records every Skill directory and instruction file
//! `install` wrote, with the hash of what it wrote. Every location is compared
//! with that record first:
//!
//! - a location the state does not record is **unmanaged** (written by hand or
//!   by another tool such as `npx skills`): it is never modified or deleted,
//!   and installing over it is refused, even with `--force`;
//! - a managed location whose current hash differs from the recorded one has
//!   **manual edits**: it is not overwritten or deleted unless
//!   [`Options::force`] is set;
//! - a managed Skill that is no longer declared, or a managed location of a
//!   Destination that is no longer enabled or has moved, is **pruned**.
//!
//! All writes go through a staging entry next to the location and a rename, so
//! a failure never leaves a half-written Skill or file. Nothing is written in
//! a dry run ([`Options::dry_run`]), not even the state.

pub mod fragments;
pub mod harness;
pub mod state;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, Permissions};
use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use crate::adaptation::SkillTree;
use crate::collection::machine::ResolvedDestination;
use crate::collection::{Collection, Entry, LOCK_FILE, SKILLS_DIR, inspect};
use crate::hash::{self, Digest};
use crate::safety::Name;
use state::{InstallState, StateError};

/// Prefix of the staging entries created next to a location while it is
/// written. They are removed before `install` returns.
const STAGING_PREFIX: &str = ".skill-curator-";

/// How `install` runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Report what would change without writing anything.
    pub dry_run: bool,
    /// Overwrite or delete managed locations that have manual edits. Unmanaged
    /// locations are never touched, even with `force`.
    pub force: bool,
}

/// Failure of `install` as a whole.
#[derive(Debug)]
#[non_exhaustive]
pub enum InstallError {
    /// The ownership state could not be read or written.
    State(StateError),
    /// The state file would be written inside an enabled Destination.
    StateInDestination {
        /// State file.
        state: PathBuf,
        /// The Destination.
        destination: Name,
    },
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(err) => err.fmt(f),
            Self::StateInDestination { state, destination } => write!(
                f,
                "the install state {} would be written inside Destination `{destination}`, where harnesses read it; point `XDG_STATE_HOME` elsewhere",
                state.display()
            ),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(err) => Some(err),
            Self::StateInDestination { .. } => None,
        }
    }
}

impl From<StateError> for InstallError {
    fn from(err: StateError) -> Self {
        Self::State(err)
    }
}

/// What an [`Item`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// The Skill directory `<dir>/<name>`: in a Destination, or in the
    /// Collection's `skills/` for a Skill that is not accepted.
    Skill {
        /// Skill name.
        name: Name,
        /// Directory holding the Skill directory.
        dir: PathBuf,
    },
    /// A global instruction file.
    InstructionFile {
        /// The file.
        path: PathBuf,
    },
    /// A Destination's Skills directory as a whole.
    SkillsDir {
        /// The directory.
        path: PathBuf,
    },
}

impl Location {
    /// The path of the Skill directory, file, or directory.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Skill { name, dir } => dir.join(name.as_str()),
            Self::InstructionFile { path } | Self::SkillsDir { path } => path.clone(),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Skill { name, .. } => {
                write!(f, "Skill `{name}` at {}", self.path().display())
            }
            Self::InstructionFile { path } => {
                write!(f, "the instruction file {}", path.display())
            }
            Self::SkillsDir { path } => write!(f, "the Skills directory {}", path.display()),
        }
    }
}

/// What happened to one location, or would happen in a dry run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Written where nothing was.
    Added,
    /// Replaced a managed copy.
    Updated {
        /// Whether manual edits were overwritten because of `--force`.
        forced: bool,
    },
    /// A managed copy that is no longer wanted was deleted.
    Removed {
        /// Whether manual edits were deleted because of `--force`.
        forced: bool,
    },
    /// A managed copy that is no longer wanted was already gone; the state
    /// forgets it.
    Forgotten,
    /// Already up to date.
    Unchanged,
    /// A managed copy has manual edits; nothing was written.
    ManualEdits {
        /// What differs.
        reason: String,
        /// Whether the copy was to be deleted rather than replaced.
        removal: bool,
    },
    /// The location exists but was not written by `skill-curator`; it is left
    /// untouched.
    Unmanaged {
        /// What is there.
        reason: String,
    },
    /// The Skill in the Collection is not accepted, so it is installed
    /// nowhere; installed copies are kept.
    NotAccepted {
        /// Why.
        reason: String,
    },
    /// The location could not be planned or written.
    Failed {
        /// Why.
        reason: String,
    },
}

impl Outcome {
    /// Whether the location still needs the user's attention.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(
            self,
            Self::ManualEdits { .. }
                | Self::Unmanaged { .. }
                | Self::NotAccepted { .. }
                | Self::Failed { .. }
        )
    }
}

/// One location and its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Destination of the location; `None` for a Skill of the Collection that
    /// is not accepted.
    pub destination: Option<Name>,
    /// What the item is about.
    pub location: Location,
    /// What happened, or would happen in a dry run.
    pub outcome: Outcome,
}

impl Item {
    /// One-line description, with what to do next when the location needs
    /// attention. Unchanged locations are described too, although the command
    /// line only counts them.
    #[must_use]
    pub fn describe(&self, dry_run: bool) -> String {
        let prefix = self
            .destination
            .as_ref()
            .map(|destination| format!("{destination}: "))
            .unwrap_or_default();
        let location = &self.location;
        let verb = |done: &str, planned: &str| if dry_run { planned } else { done }.to_owned();
        let text = match &self.outcome {
            Outcome::Added => format!("{} {location}", verb("added", "would add")),
            Outcome::Updated { forced } => format!(
                "{} {location}{}",
                verb("updated", "would update"),
                if *forced {
                    ", overwriting its manual edits (--force)"
                } else {
                    ""
                }
            ),
            Outcome::Removed { forced } => format!(
                "{} {location}, which is no longer declared there{}",
                verb("removed", "would remove"),
                if *forced {
                    ", deleting its manual edits (--force)"
                } else {
                    ""
                }
            ),
            Outcome::Forgotten => format!(
                "{location} is already gone; {} it",
                verb("forgot", "would forget")
            ),
            Outcome::Unchanged => format!("{location} is unchanged"),
            Outcome::ManualEdits { reason, removal } => {
                let (action, fix) = if *removal {
                    ("deleted", "delete")
                } else {
                    ("overwritten", "overwrite")
                };
                format!(
                    "{location} has manual edits ({reason}); it is not {action}; keep the edits in the Collection instead, or run `skill-curator install --force` to {fix} them"
                )
            }
            Outcome::Unmanaged { reason } => {
                let fix = match location {
                    Location::InstructionFile { .. } => {
                        "move its content into a Fragment and remove the file to let skill-curator manage it"
                    }
                    Location::Skill { .. } | Location::SkillsDir { .. } => {
                        "it may come from `npx skills` or a manual install; remove or rename it, or rename the Skill in manifest.toml"
                    }
                };
                format!(
                    "{location} is not managed by skill-curator ({reason}); refusing to overwrite it, and it is left untouched; {fix}"
                )
            }
            Outcome::NotAccepted { reason } => match location {
                Location::Skill { name, .. } => {
                    format!("Skill `{name}` is not installed: {reason}")
                }
                Location::InstructionFile { .. } | Location::SkillsDir { .. } => {
                    format!("{location} is not installed: {reason}")
                }
            },
            Outcome::Failed { reason } => format!("{location}: failed: {reason}"),
        };
        format!("{prefix}{text}")
    }
}

/// A warning about a Skill as installed in one Destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// The Destination.
    pub destination: Name,
    /// The Skill.
    pub skill: Name,
    /// What to look at.
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: Skill `{}`: {}",
            self.destination, self.skill, self.message
        )
    }
}

/// The outcome of every location, and the warnings found on the way.
#[derive(Debug, Default)]
pub struct Report {
    /// Whether nothing was written.
    pub dry_run: bool,
    /// Outcomes, Destination by Destination, then pruned locations of
    /// Destinations no longer enabled.
    pub items: Vec<Item>,
    /// Warnings about installed Skills.
    pub warnings: Vec<Warning>,
}

impl Report {
    /// Number of locations that still need the user's attention.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.outcome.is_pending())
            .count()
    }

    /// One-line count of the changes.
    #[must_use]
    pub fn summary(&self) -> String {
        let count = |wanted: fn(&Outcome) -> bool| {
            self.items
                .iter()
                .filter(|item| wanted(&item.outcome))
                .count()
        };
        let added = count(|outcome| matches!(outcome, Outcome::Added));
        let updated = count(|outcome| matches!(outcome, Outcome::Updated { .. }));
        let removed = count(|outcome| matches!(outcome, Outcome::Removed { .. }));
        let unchanged = count(|outcome| matches!(outcome, Outcome::Unchanged));
        let pending = self.pending();
        let mut text = format!(
            "{added} added, {updated} updated, {removed} removed, {unchanged} unchanged, {pending} needing attention"
        );
        if self.dry_run {
            text.push_str(" (dry run: nothing was written)");
        }
        text
    }
}

/// Runs `install`: installs the accepted Skills of `collection` and its
/// assembled Fragments into the enabled `destinations`, recording ownership
/// in the state file at `state_path`.
///
/// # Errors
///
/// Returns an [`InstallError`] when the state file lies inside an enabled
/// Destination, or cannot be read or written. Problems with single locations
/// are reported as pending [`Outcome`]s instead, after every other location is
/// processed.
pub fn install(
    collection: &Collection,
    destinations: &[ResolvedDestination],
    state_path: &Path,
    options: Options,
) -> Result<Report, InstallError> {
    check_state_path(state_path, destinations)?;
    let mut state = InstallState::load(state_path)?;
    let mut report = Report {
        dry_run: options.dry_run,
        ..Report::default()
    };
    let (steps, warnings) = Planner {
        collection,
        state: &state,
        force: options.force,
        steps: Vec::new(),
        warnings: Vec::new(),
    }
    .plan(destinations);
    report.warnings = warnings;
    for step in steps {
        let outcome = if options.dry_run {
            step.kind.planned()
        } else {
            apply(&step, &mut state, state_path)?
        };
        report.items.push(Item {
            destination: step.destination,
            location: step.location,
            outcome,
        });
    }
    Ok(report)
}

fn check_state_path(
    state_path: &Path,
    destinations: &[ResolvedDestination],
) -> Result<(), InstallError> {
    for destination in destinations.iter().filter(|d| d.enabled) {
        let inside_skills = state_path.starts_with(&destination.skills_dir);
        let is_instructions = destination
            .instructions
            .as_ref()
            .is_some_and(|instructions| state_path.starts_with(&instructions.file));
        if inside_skills || is_instructions {
            return Err(InstallError::StateInDestination {
                state: state_path.to_path_buf(),
                destination: destination.name.clone(),
            });
        }
    }
    Ok(())
}

/// Content to write at a location.
enum Payload {
    Skill(SkillTree),
    File(Vec<u8>),
}

/// What to do with one location.
enum StepKind {
    Add {
        payload: Payload,
        digest: Digest,
    },
    Update {
        payload: Payload,
        digest: Digest,
        forced: bool,
    },
    Remove {
        forced: bool,
    },
    Forget,
    Unchanged {
        digest: Digest,
    },
    Blocked(Outcome),
}

impl StepKind {
    fn planned(self) -> Outcome {
        match self {
            Self::Add { .. } => Outcome::Added,
            Self::Update { forced, .. } => Outcome::Updated { forced },
            Self::Remove { forced } => Outcome::Removed { forced },
            Self::Forget => Outcome::Forgotten,
            Self::Unchanged { .. } => Outcome::Unchanged,
            Self::Blocked(outcome) => outcome,
        }
    }
}

struct Step {
    destination: Option<Name>,
    location: Location,
    kind: StepKind,
}

/// What exists at a location.
enum Found {
    Missing,
    Present(Digest),
    Unreadable(String),
}

impl Found {
    /// What differs from the recorded copy, for a [`Outcome::ManualEdits`].
    fn edit_reason(self, what: &str) -> String {
        match self {
            Self::Unreadable(reason) => reason,
            Self::Missing | Self::Present(_) => {
                format!("its {what} differ from the copy skill-curator installed")
            }
        }
    }
}

/// The Skill directory at `path`, hashed without following links.
fn find_skill(path: &Path) -> Found {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Found::Missing,
        Err(err) => Found::Unreadable(format!("cannot read it: {err}")),
        Ok(_) => match hash::tree_digest(path) {
            Ok(digest) => Found::Present(digest),
            Err(err) => Found::Unreadable(err.to_string()),
        },
    }
}

/// The file at `path`, hashed without following links.
fn find_file(path: &Path) -> Found {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Found::Missing,
        Err(err) => Found::Unreadable(format!("cannot read it: {err}")),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Found::Unreadable("it is a symbolic link".to_owned())
        }
        Ok(metadata) if metadata.is_file() => match fs::read(path) {
            Ok(contents) => Found::Present(hash::file_digest(&contents)),
            Err(err) => Found::Unreadable(format!("cannot read it: {err}")),
        },
        Ok(_) => Found::Unreadable("it is not a regular file".to_owned()),
    }
}

/// Why `install` cannot write below the Skills directory `dir`, if it
/// cannot. A missing directory is created when a Skill is added.
fn skills_dir_problem(dir: &Path) -> Option<String> {
    match fs::symlink_metadata(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => Some(format!("cannot read it: {err}")),
        Ok(metadata) if metadata.file_type().is_symlink() => Some(
            "it is a symbolic link; skill-curator writes only into a real directory, so that no write lands outside the configured Destination; configure the directory the link points to instead".to_owned(),
        ),
        Ok(metadata) if metadata.is_dir() => None,
        Ok(_) => Some("it is not a directory".to_owned()),
    }
}

/// Plans writing `payload` (hashing to `digest`) at a location, given its
/// recorded hash and what is there now.
fn plan_write(
    managed: Option<&Digest>,
    found: Found,
    payload: Payload,
    digest: Digest,
    force: bool,
    what: &str,
) -> StepKind {
    match (managed, found) {
        (_, Found::Missing) => StepKind::Add { payload, digest },
        (None, Found::Present(_)) => StepKind::Blocked(Outcome::Unmanaged {
            reason: "it already exists".to_owned(),
        }),
        (None, Found::Unreadable(reason)) => StepKind::Blocked(Outcome::Unmanaged { reason }),
        (Some(_), Found::Present(current)) if current == digest => StepKind::Unchanged { digest },
        (Some(recorded), Found::Present(current)) if current == *recorded => StepKind::Update {
            payload,
            digest,
            forced: false,
        },
        (Some(_), _) if force => StepKind::Update {
            payload,
            digest,
            forced: true,
        },
        (Some(_), found) => StepKind::Blocked(Outcome::ManualEdits {
            reason: found.edit_reason(what),
            removal: false,
        }),
    }
}

/// Plans pruning a managed location recorded with `recorded`.
fn plan_removal(recorded: &Digest, found: Found, force: bool, what: &str) -> StepKind {
    match found {
        Found::Missing => StepKind::Forget,
        Found::Present(current) if current == *recorded => StepKind::Remove { forced: false },
        _ if force => StepKind::Remove { forced: true },
        found => StepKind::Blocked(Outcome::ManualEdits {
            reason: found.edit_reason(what),
            removal: true,
        }),
    }
}

/// The accepted Skills of a Collection, or why each is not accepted.
fn accepted_skills(collection: &Collection) -> BTreeMap<Name, Result<SkillTree, String>> {
    collection
        .manifest()
        .skills
        .keys()
        .map(|name| (name.clone(), accepted_skill(collection, name)))
        .collect()
}

fn accepted_skill(collection: &Collection, name: &Name) -> Result<SkillTree, String> {
    let dir = format!("{SKILLS_DIR}/{name}/");
    let Some(locked) = collection.lock().skills.get(name) else {
        return Err(
            "it is not materialised yet; run `skill-curator update` to accept it, or `skill-curator materialize` for a local Skill"
                .to_owned(),
        );
    };
    match inspect(collection.root(), &[SKILLS_DIR, name.as_str()]) {
        Ok(Entry::Dir) => {}
        Ok(Entry::Symlink(link)) => {
            return Err(format!(
                "`{dir}` is reached through the symbolic link {}",
                link.display()
            ));
        }
        Ok(Entry::Missing | Entry::File | Entry::Other) => {
            return Err(format!("`{dir}` is not a directory of the Collection"));
        }
        Err(err) => return Err(err.to_string()),
    }
    let tree = SkillTree::read(&collection.skill_dir(name)).map_err(|err| err.to_string())?;
    if tree.digest() != locked.output_hash {
        return Err(format!(
            "`{dir}` differs from the accepted output recorded in {LOCK_FILE}; run `skill-curator materialize` to accept or reset the edits"
        ));
    }
    Ok(tree)
}

struct Planner<'a> {
    collection: &'a Collection,
    state: &'a InstallState,
    force: bool,
    steps: Vec<Step>,
    warnings: Vec<Warning>,
}

impl Planner<'_> {
    fn plan(mut self, destinations: &[ResolvedDestination]) -> (Vec<Step>, Vec<Warning>) {
        let accepted = accepted_skills(self.collection);
        for (name, result) in &accepted {
            if let Err(reason) = result {
                self.steps.push(Step {
                    destination: None,
                    location: Location::Skill {
                        name: name.clone(),
                        dir: self.collection.root().join(SKILLS_DIR),
                    },
                    kind: StepKind::Blocked(Outcome::NotAccepted {
                        reason: reason.clone(),
                    }),
                });
            }
        }
        let enabled = destinations
            .iter()
            .filter(|destination| destination.enabled)
            .collect::<Vec<_>>();
        for destination in &enabled {
            self.plan_skills(destination, &accepted);
            self.plan_instructions(destination);
        }
        self.plan_stale(&enabled);
        (self.steps, self.warnings)
    }

    fn push(&mut self, destination: &Name, location: Location, kind: StepKind) {
        self.steps.push(Step {
            destination: Some(destination.clone()),
            location,
            kind,
        });
    }

    fn plan_skills(
        &mut self,
        destination: &ResolvedDestination,
        accepted: &BTreeMap<Name, Result<SkillTree, String>>,
    ) {
        let dir = &destination.skills_dir;
        if let Some(reason) = skills_dir_problem(dir) {
            self.push(
                &destination.name,
                Location::SkillsDir { path: dir.clone() },
                StepKind::Blocked(Outcome::Failed { reason }),
            );
            return;
        }
        for (name, result) in accepted {
            let Ok(skill) = result else { continue };
            let location = Location::Skill {
                name: name.clone(),
                dir: dir.clone(),
            };
            let kind = match harness::adapt(destination.harness, &destination.frontmatter, skill) {
                Ok(adapted) => {
                    self.warnings
                        .extend(adapted.warnings.into_iter().map(|message| Warning {
                            destination: destination.name.clone(),
                            skill: name.clone(),
                            message,
                        }));
                    let digest = adapted.tree.digest();
                    plan_write(
                        self.state.skill(dir, name),
                        find_skill(&location.path()),
                        Payload::Skill(adapted.tree),
                        digest,
                        self.force,
                        "files",
                    )
                }
                Err(err) => StepKind::Blocked(Outcome::Failed {
                    reason: err.to_string(),
                }),
            };
            self.push(&destination.name, location, kind);
        }
        // Declared Skills that are not accepted keep their installed copy.
        if let Some(managed) = self.state.skills_dirs.get(dir) {
            for (name, recorded) in &managed.skills {
                if !accepted.contains_key(name) {
                    self.plan_skill_removal(&destination.name, dir, name, recorded);
                }
            }
        }
    }

    fn plan_skill_removal(
        &mut self,
        destination: &Name,
        dir: &Path,
        name: &Name,
        recorded: &Digest,
    ) {
        let location = Location::Skill {
            name: name.clone(),
            dir: dir.to_path_buf(),
        };
        let kind = plan_removal(recorded, find_skill(&location.path()), self.force, "files");
        self.push(destination, location, kind);
    }

    fn plan_instructions(&mut self, destination: &ResolvedDestination) {
        let Some(instructions) = &destination.instructions else {
            return;
        };
        let location = Location::InstructionFile {
            path: instructions.file.clone(),
        };
        let kind = match fragments::assemble(self.collection.root(), &instructions.fragments) {
            Ok(contents) => {
                let digest = hash::file_digest(&contents);
                plan_write(
                    self.state.instruction_file(&instructions.file),
                    find_file(&instructions.file),
                    Payload::File(contents),
                    digest,
                    self.force,
                    "contents",
                )
            }
            Err(err) => StepKind::Blocked(Outcome::Failed {
                reason: err.to_string(),
            }),
        };
        self.push(&destination.name, location, kind);
    }

    /// Prunes managed locations that no enabled Destination writes any more:
    /// the Destination was disabled, removed, or moved to another path.
    fn plan_stale(&mut self, enabled: &[&ResolvedDestination]) {
        let skills_dirs = enabled
            .iter()
            .map(|destination| destination.skills_dir.as_path())
            .collect::<BTreeSet<_>>();
        let state = self.state;
        for (dir, managed) in &state.skills_dirs {
            if skills_dirs.contains(dir.as_path()) {
                continue;
            }
            if let Some(reason) = skills_dir_problem(dir) {
                self.push(
                    &managed.destination,
                    Location::SkillsDir { path: dir.clone() },
                    StepKind::Blocked(Outcome::Failed { reason }),
                );
                continue;
            }
            for (name, recorded) in &managed.skills {
                self.plan_skill_removal(&managed.destination, dir, name, recorded);
            }
        }
        let files = enabled
            .iter()
            .filter_map(|destination| destination.instructions.as_ref())
            .map(|instructions| instructions.file.as_path())
            .collect::<BTreeSet<_>>();
        for (path, managed) in &state.instruction_files {
            if files.contains(path.as_path()) {
                continue;
            }
            let kind = plan_removal(&managed.hash, find_file(path), self.force, "contents");
            self.push(
                &managed.destination,
                Location::InstructionFile { path: path.clone() },
                kind,
            );
        }
    }
}

/// Carries out one step, saving the state around each write so that it
/// always records what `install` owns: an added location is recorded before it
/// is written, a replaced or removed one after.
fn apply(step: &Step, state: &mut InstallState, state_path: &Path) -> Result<Outcome, StateError> {
    let path = step.location.path();
    let outcome = match &step.kind {
        StepKind::Blocked(outcome) => outcome.clone(),
        StepKind::Unchanged { digest } => {
            if recorded(state, &step.location) != Some(digest) {
                record(state, step, digest.clone());
                state.save(state_path)?;
            }
            Outcome::Unchanged
        }
        StepKind::Add { payload, digest } => {
            record(state, step, digest.clone());
            state.save(state_path)?;
            match add(&path, payload) {
                Ok(()) => Outcome::Added,
                Err(reason) => {
                    forget(state, &step.location);
                    state.save(state_path)?;
                    Outcome::Failed { reason }
                }
            }
        }
        StepKind::Update {
            payload,
            digest,
            forced,
        } => match replace(&path, payload) {
            Ok(()) => {
                record(state, step, digest.clone());
                state.save(state_path)?;
                Outcome::Updated { forced: *forced }
            }
            Err(reason) => Outcome::Failed { reason },
        },
        StepKind::Remove { forced } => match remove(&step.location) {
            Ok(()) => {
                forget(state, &step.location);
                state.save(state_path)?;
                Outcome::Removed { forced: *forced }
            }
            Err(reason) => Outcome::Failed { reason },
        },
        StepKind::Forget => {
            forget(state, &step.location);
            state.save(state_path)?;
            Outcome::Forgotten
        }
    };
    Ok(outcome)
}

fn recorded<'a>(state: &'a InstallState, location: &Location) -> Option<&'a Digest> {
    match location {
        Location::Skill { name, dir } => state.skill(dir, name),
        Location::InstructionFile { path } => state.instruction_file(path),
        Location::SkillsDir { .. } => None,
    }
}

fn record(state: &mut InstallState, step: &Step, digest: Digest) {
    let Some(destination) = &step.destination else {
        return;
    };
    match &step.location {
        Location::Skill { name, dir } => state.record_skill(destination, dir, name, digest),
        Location::InstructionFile { path } => {
            state.record_instruction_file(destination, path, digest);
        }
        Location::SkillsDir { .. } => {}
    }
}

fn forget(state: &mut InstallState, location: &Location) {
    match location {
        Location::Skill { name, dir } => state.forget_skill(dir, name),
        Location::InstructionFile { path } => state.forget_instruction_file(path),
        Location::SkillsDir { .. } => {}
    }
}

/// The directory holding `path`, created if missing.
fn parent_dir(path: &Path) -> Result<&Path, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    Ok(parent)
}

/// Writes `payload` at `path`, where nothing exists; never replaces what
/// appeared there since the plan.
fn add(path: &Path, payload: &Payload) -> Result<(), String> {
    let parent = parent_dir(path)?;
    match payload {
        Payload::Skill(tree) => {
            let staging = tempfile::Builder::new()
                .prefix(STAGING_PREFIX)
                .tempdir_in(parent)
                .map_err(|err| format!("cannot write in {}: {err}", parent.display()))?;
            let new = staging.path().join("new");
            tree.write_new(&new).map_err(|err| err.to_string())?;
            if fs::symlink_metadata(path).is_ok() {
                return Err(format!(
                    "{} appeared while installing; it is left untouched",
                    path.display()
                ));
            }
            fs::rename(&new, path).map_err(|err| format!("cannot write {}: {err}", path.display()))
        }
        Payload::File(contents) => write_file(path, contents, false)
            .map_err(|err| format!("cannot write {}: {err}", path.display())),
    }
}

/// Replaces whatever is at `path` (a directory, file, or symbolic link,
/// never followed) with `payload`.
fn replace(path: &Path, payload: &Payload) -> Result<(), String> {
    parent_dir(path)?;
    match payload {
        Payload::Skill(tree) => tree
            .replace(path, STAGING_PREFIX)
            .map_err(|err| err.to_string()),
        Payload::File(contents) => write_file(path, contents, true)
            .map_err(|err| format!("cannot write {}: {err}", path.display())),
    }
}

/// Deletes the location, never following a symbolic link: a Skill directory is
/// moved into a staging directory next to it, which is then deleted.
fn remove(location: &Location) -> Result<(), String> {
    let path = location.path();
    let error = |err: io::Error| format!("cannot remove {}: {err}", path.display());
    match location {
        Location::Skill { dir, .. } => {
            let staging = tempfile::Builder::new()
                .prefix(STAGING_PREFIX)
                .tempdir_in(dir)
                .map_err(error)?;
            fs::rename(&path, staging.path().join("old")).map_err(error)?;
            staging.close().map_err(error)
        }
        Location::InstructionFile { .. } | Location::SkillsDir { .. } => {
            fs::remove_file(&path).map_err(error)
        }
    }
}

/// Writes `contents` to `path` through a temporary file in the same
/// directory renamed into place, so the file is never half-written and a
/// symbolic link at `path` is replaced rather than followed. Without
/// `replace`, fails if `path` exists. The file gets mode `0644`.
fn write_file(path: &Path, contents: &[u8], replace: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("the path has no parent directory"))?;
    let mut file = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempfile_in(parent)?;
    file.write_all(contents)?;
    file.as_file()
        .set_permissions(Permissions::from_mode(0o644))?;
    let persisted = if replace {
        file.persist(path)
    } else {
        file.persist_noclobber(path)
    };
    persisted.map(drop).map_err(|err| err.error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::file_digest;

    fn digest(text: &str) -> Digest {
        file_digest(text.as_bytes())
    }

    fn payload() -> Payload {
        Payload::File(Vec::new())
    }

    fn outcome(kind: StepKind) -> Outcome {
        kind.planned()
    }

    #[test]
    fn writes_are_planned_from_the_recorded_and_current_hashes() {
        let (old, new, edited) = (digest("old"), digest("new"), digest("edited"));
        let plan = |managed: Option<&Digest>, found, force| {
            outcome(plan_write(
                managed,
                found,
                payload(),
                new.clone(),
                force,
                "files",
            ))
        };
        assert_eq!(plan(None, Found::Missing, false), Outcome::Added);
        assert_eq!(plan(Some(&old), Found::Missing, false), Outcome::Added);
        assert_eq!(
            plan(Some(&old), Found::Present(new.clone()), false),
            Outcome::Unchanged
        );
        assert_eq!(
            plan(Some(&old), Found::Present(old.clone()), false),
            Outcome::Updated { forced: false }
        );
        assert_eq!(
            plan(Some(&old), Found::Present(edited.clone()), false),
            Outcome::ManualEdits {
                reason: "its files differ from the copy skill-curator installed".to_owned(),
                removal: false
            }
        );
        assert_eq!(
            plan(Some(&old), Found::Present(edited.clone()), true),
            Outcome::Updated { forced: true }
        );
        assert_eq!(
            plan(
                Some(&old),
                Found::Unreadable("it is a symbolic link".to_owned()),
                false
            ),
            Outcome::ManualEdits {
                reason: "it is a symbolic link".to_owned(),
                removal: false
            }
        );
        // Unmanaged locations are refused, even with `force` and even when
        // they already hold exactly what would be written.
        for force in [false, true] {
            assert_eq!(
                plan(None, Found::Present(new.clone()), force),
                Outcome::Unmanaged {
                    reason: "it already exists".to_owned()
                }
            );
            assert_eq!(
                plan(
                    None,
                    Found::Unreadable("it is a symbolic link".to_owned()),
                    force
                ),
                Outcome::Unmanaged {
                    reason: "it is a symbolic link".to_owned()
                }
            );
        }
    }

    #[test]
    fn removals_delete_only_unedited_copies_unless_forced() {
        let recorded = digest("installed");
        let plan = |found, force| outcome(plan_removal(&recorded, found, force, "contents"));
        assert_eq!(plan(Found::Missing, false), Outcome::Forgotten);
        assert_eq!(
            plan(Found::Present(recorded.clone()), false),
            Outcome::Removed { forced: false }
        );
        assert_eq!(
            plan(Found::Present(digest("edited")), false),
            Outcome::ManualEdits {
                reason: "its contents differ from the copy skill-curator installed".to_owned(),
                removal: true
            }
        );
        assert_eq!(
            plan(Found::Present(digest("edited")), true),
            Outcome::Removed { forced: true }
        );
        assert_eq!(
            plan(
                Found::Unreadable("it is not a regular file".to_owned()),
                true
            ),
            Outcome::Removed { forced: true }
        );
    }

    #[test]
    fn only_unresolved_outcomes_are_pending() {
        let reason = || "why".to_owned();
        for outcome in [
            Outcome::ManualEdits {
                reason: reason(),
                removal: false,
            },
            Outcome::Unmanaged { reason: reason() },
            Outcome::NotAccepted { reason: reason() },
            Outcome::Failed { reason: reason() },
        ] {
            assert!(outcome.is_pending(), "{outcome:?}");
        }
        for outcome in [
            Outcome::Added,
            Outcome::Updated { forced: true },
            Outcome::Removed { forced: false },
            Outcome::Forgotten,
            Outcome::Unchanged,
        ] {
            assert!(!outcome.is_pending(), "{outcome:?}");
        }
    }

    #[test]
    fn descriptions_name_the_destination_target_and_next_step() {
        let name = Name::new("grill-me").unwrap();
        let skill = Location::Skill {
            name: name.clone(),
            dir: PathBuf::from("/h/.claude/skills"),
        };
        let item = |location: &Location, outcome| Item {
            destination: Some(Name::new("claude").unwrap()),
            location: location.clone(),
            outcome,
        };
        assert_eq!(
            item(&skill, Outcome::Added).describe(false),
            "claude: added Skill `grill-me` at /h/.claude/skills/grill-me"
        );
        assert_eq!(
            item(&skill, Outcome::Added).describe(true),
            "claude: would add Skill `grill-me` at /h/.claude/skills/grill-me"
        );
        assert_eq!(
            item(&skill, Outcome::Updated { forced: true }).describe(false),
            "claude: updated Skill `grill-me` at /h/.claude/skills/grill-me, overwriting its manual edits (--force)"
        );
        assert_eq!(
            item(&skill, Outcome::Removed { forced: false }).describe(true),
            "claude: would remove Skill `grill-me` at /h/.claude/skills/grill-me, which is no longer declared there"
        );
        let edited = item(
            &skill,
            Outcome::ManualEdits {
                reason: "why".to_owned(),
                removal: false,
            },
        )
        .describe(false);
        assert!(
            edited.contains("has manual edits (why); it is not overwritten"),
            "{edited}"
        );
        assert!(edited.ends_with("run `skill-curator install --force` to overwrite them"));

        let file = Location::InstructionFile {
            path: PathBuf::from("/h/.claude/CLAUDE.md"),
        };
        let unmanaged = item(
            &file,
            Outcome::Unmanaged {
                reason: "it already exists".to_owned(),
            },
        )
        .describe(false);
        assert!(
            unmanaged.starts_with("claude: the instruction file /h/.claude/CLAUDE.md is not managed by skill-curator (it already exists); refusing to overwrite it"),
            "{unmanaged}"
        );
        assert!(unmanaged.contains("move its content into a Fragment"));
        let unmanaged = item(
            &skill,
            Outcome::Unmanaged {
                reason: "it already exists".to_owned(),
            },
        )
        .describe(false);
        assert!(unmanaged.contains("`npx skills`"), "{unmanaged}");

        let not_accepted = Item {
            destination: None,
            location: Location::Skill {
                name,
                dir: PathBuf::from("/c/skills"),
            },
            outcome: Outcome::NotAccepted {
                reason: "why".to_owned(),
            },
        };
        assert_eq!(
            not_accepted.describe(false),
            "Skill `grill-me` is not installed: why"
        );
    }

    #[test]
    fn summary_counts_changes_and_marks_dry_runs() {
        let item = |outcome| Item {
            destination: None,
            location: Location::InstructionFile {
                path: PathBuf::from("/f"),
            },
            outcome,
        };
        let mut report = Report {
            dry_run: false,
            items: vec![
                item(Outcome::Added),
                item(Outcome::Added),
                item(Outcome::Updated { forced: false }),
                item(Outcome::Removed { forced: true }),
                item(Outcome::Unchanged),
                item(Outcome::Failed {
                    reason: "why".to_owned(),
                }),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            report.summary(),
            "2 added, 1 updated, 1 removed, 1 unchanged, 1 needing attention"
        );
        assert_eq!(report.pending(), 1);
        report.dry_run = true;
        assert!(
            report
                .summary()
                .ends_with(" (dry run: nothing was written)")
        );
    }
}
