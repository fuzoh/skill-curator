//! D2: relative path globs selecting which files of an upstream Skill are
//! kept.
//!
//! A file is kept when every rule set keeps it, in order: a rule set keeps a
//! file when its `include` globs (if any) select it and none of its
//! `exclude` globs do. A glob selects a file when it matches the file's
//! relative path or the path of one of the directories holding it, so
//! `evals`, `evals/`, and `evals/**` all select every file under `evals/`;
//! a glob ending in `/` only matches directories. `*` and `?` never match
//! `/`, `**` matches any number of directories, and `\` escapes the next
//! character. Matching is case-sensitive.

use std::collections::BTreeSet;
use std::fmt;

use globset::{GlobBuilder, GlobMatcher};

use crate::collection::SKILL_FILE;
use crate::collection::manifest::FileRules;
use crate::safety::{PathPattern, RelativePath};

/// Why a D2 rule set cannot be used or drops `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilesError {
    /// A pattern is not a valid glob.
    InvalidPattern {
        /// The pattern as written.
        pattern: PathPattern,
        /// What is wrong with it.
        message: String,
    },
    /// The Skill has no `SKILL.md` at its top.
    MissingSkillFile,
    /// The rules drop the Skill's `SKILL.md`, which every Skill must keep.
    SkillFileDropped {
        /// Which rule drops it.
        reason: Dropped,
    },
}

impl fmt::Display for FilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPattern { pattern, message } => {
                write!(f, "`{pattern}` is not a valid glob: {message}")
            }
            Self::MissingSkillFile => write!(f, "the Skill has no `{SKILL_FILE}` at its top"),
            Self::SkillFileDropped { reason } => write!(
                f,
                "the file rules (D2) drop `{SKILL_FILE}`, which every Skill must keep: {reason}; narrow the rule so it keeps `{SKILL_FILE}`"
            ),
        }
    }
}

impl std::error::Error for FilesError {}

/// Which rule drops a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    /// An `exclude` glob selects it.
    Excluded(PathPattern),
    /// An `include` list exists and none of its globs select it.
    NotIncluded,
}

impl fmt::Display for Dropped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Excluded(pattern) => write!(f, "`exclude` pattern `{pattern}` selects it"),
            Self::NotIncluded => f.write_str("no `include` pattern selects it"),
        }
    }
}

/// Why `pattern` is not a valid glob, if it is not.
#[must_use]
pub fn pattern_problem(pattern: &PathPattern) -> Option<String> {
    compile(pattern).err().map(|err| err.to_string())
}

fn compile(pattern: &PathPattern) -> Result<GlobMatcher, globset::Error> {
    Ok(GlobBuilder::new(pattern.as_str())
        .literal_separator(true)
        .backslash_escape(true)
        .build()?
        .compile_matcher())
}

/// A compiled glob and the pattern it came from.
#[derive(Debug, Clone)]
struct Glob {
    pattern: PathPattern,
    matcher: GlobMatcher,
}

impl Glob {
    fn new(pattern: &PathPattern) -> Result<Self, FilesError> {
        let matcher = compile(pattern).map_err(|err| FilesError::InvalidPattern {
            pattern: pattern.clone(),
            message: err.to_string(),
        })?;
        Ok(Self {
            pattern: pattern.clone(),
            matcher,
        })
    }

    /// Whether the glob selects the file at `path`: matches the path itself,
    /// or one of its parent directories written with or without a trailing
    /// `/`.
    fn selects(&self, path: &RelativePath) -> bool {
        let path = path.as_str();
        if self.matcher.is_match(path) {
            return true;
        }
        path.match_indices('/').any(|(end, _)| {
            self.matcher.is_match(&path[..end]) || self.matcher.is_match(&path[..=end])
        })
    }
}

/// One compiled D2 rule set.
#[derive(Debug, Clone)]
struct Stage {
    exclude: Vec<Glob>,
    include: Option<Vec<Glob>>,
}

impl Stage {
    fn dropped(&self, path: &RelativePath) -> Option<Dropped> {
        if let Some(include) = &self.include
            && !include.iter().any(|glob| glob.selects(path))
        {
            return Some(Dropped::NotIncluded);
        }
        self.exclude
            .iter()
            .find(|glob| glob.selects(path))
            .map(|glob| Dropped::Excluded(glob.pattern.clone()))
    }
}

/// D2 rule sets compiled for matching, applied in order.
#[derive(Debug, Clone)]
pub struct FileFilter {
    stages: Vec<Stage>,
}

impl FileFilter {
    /// Compiles `rules`, applied in the given order.
    ///
    /// # Errors
    ///
    /// Returns [`FilesError::InvalidPattern`] for the first pattern that is
    /// not a valid glob.
    pub fn new(rules: &[&FileRules]) -> Result<Self, FilesError> {
        let compile_all =
            |patterns: &[PathPattern]| patterns.iter().map(Glob::new).collect::<Result<_, _>>();
        let stages = rules
            .iter()
            .map(|rules| {
                Ok(Stage {
                    exclude: compile_all(&rules.exclude)?,
                    include: rules.include.as_deref().map(compile_all).transpose()?,
                })
            })
            .collect::<Result<_, FilesError>>()?;
        Ok(Self { stages })
    }

    /// Why the rules drop `path`, or `None` when they keep it.
    #[must_use]
    pub fn dropped(&self, path: &RelativePath) -> Option<Dropped> {
        self.stages.iter().find_map(|stage| stage.dropped(path))
    }

    /// Whether the rules keep `path`.
    #[must_use]
    pub fn keeps(&self, path: &RelativePath) -> bool {
        self.dropped(path).is_none()
    }

    /// The paths of `files` the rules keep, which must include the Skill's
    /// top-level `SKILL.md`.
    ///
    /// # Errors
    ///
    /// Returns [`FilesError::MissingSkillFile`] when `files` has no
    /// `SKILL.md`, or [`FilesError::SkillFileDropped`] when the rules drop
    /// it.
    pub fn select<'a>(
        &self,
        files: impl IntoIterator<Item = &'a RelativePath>,
    ) -> Result<BTreeSet<RelativePath>, FilesError> {
        let mut has_skill_file = false;
        let mut kept = BTreeSet::new();
        for path in files {
            match self.dropped(path) {
                None => {
                    kept.insert(path.clone());
                }
                Some(reason) if path.as_str() == SKILL_FILE => {
                    return Err(FilesError::SkillFileDropped { reason });
                }
                Some(_) => {}
            }
            has_skill_file |= path.as_str() == SKILL_FILE;
        }
        if has_skill_file {
            Ok(kept)
        } else {
            Err(FilesError::MissingSkillFile)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(raw: &[&str]) -> Vec<PathPattern> {
        raw.iter().map(|p| PathPattern::new(p).unwrap()).collect()
    }

    fn file_rules(exclude: &[&str], include: Option<&[&str]>) -> FileRules {
        FileRules {
            exclude: patterns(exclude),
            include: include.map(patterns),
        }
    }

    fn paths(raw: &[&str]) -> Vec<RelativePath> {
        raw.iter()
            .map(|p| RelativePath::exact(p).unwrap())
            .collect()
    }

    fn kept(rules: &[&FileRules], files: &[&str]) -> Vec<String> {
        FileFilter::new(rules)
            .unwrap()
            .select(&paths(files))
            .unwrap()
            .into_iter()
            .map(String::from)
            .collect()
    }

    const FILES: &[&str] = &[
        "SKILL.md",
        "README.md",
        "evals/basic.md",
        "evals/deep/case.json",
        "references/guide.md",
        "references/evals/note.md",
        "scripts/run.sh",
        "logo.png",
        "assets/logo.png",
    ];

    #[test]
    fn no_rules_keep_every_file() {
        assert_eq!(kept(&[], FILES).len(), FILES.len());
        assert_eq!(kept(&[&FileRules::default()], FILES).len(), FILES.len());
    }

    #[test]
    fn exclude_globs_drop_matching_files_and_directories() {
        for pattern in ["evals", "evals/", "evals/**"] {
            assert_eq!(
                kept(&[&file_rules(&[pattern], None)], FILES),
                [
                    "README.md",
                    "SKILL.md",
                    "assets/logo.png",
                    "logo.png",
                    "references/evals/note.md",
                    "references/guide.md",
                    "scripts/run.sh",
                ],
                "{pattern}"
            );
        }
    }

    #[test]
    fn single_stars_stay_within_one_directory() {
        assert_eq!(
            kept(&[&file_rules(&["*.png"], None)], FILES),
            [
                "README.md",
                "SKILL.md",
                "assets/logo.png",
                "evals/basic.md",
                "evals/deep/case.json",
                "references/evals/note.md",
                "references/guide.md",
                "scripts/run.sh",
            ]
        );
        assert_eq!(
            kept(&[&file_rules(&["**/*.png", "**/evals"], None)], FILES),
            [
                "README.md",
                "SKILL.md",
                "references/guide.md",
                "scripts/run.sh",
            ]
        );
    }

    #[test]
    fn include_keeps_only_selected_files_and_exclude_still_applies() {
        assert_eq!(
            kept(
                &[&file_rules(
                    &["references/evals"],
                    Some(&["SKILL.md", "references"])
                )],
                FILES
            ),
            ["SKILL.md", "references/guide.md"]
        );
    }

    #[test]
    fn a_trailing_slash_matches_directories_only() {
        let files = ["SKILL.md", "notes", "docs/notes"];
        assert_eq!(
            kept(&[&file_rules(&["notes/"], None)], &files),
            ["SKILL.md", "docs/notes", "notes"]
        );
        assert_eq!(
            kept(&[&file_rules(&["**/notes"], None)], &files),
            ["SKILL.md"]
        );
    }

    #[test]
    fn rule_sets_apply_in_order_and_each_must_keep_a_file() {
        let preset = file_rules(&["evals"], None);
        let own = file_rules(&[], Some(&["SKILL.md", "evals/basic.md", "scripts"]));
        assert_eq!(
            kept(&[&preset, &own], FILES),
            ["SKILL.md", "scripts/run.sh"]
        );
    }

    #[test]
    fn globs_are_case_sensitive_and_support_escapes_and_alternatives() {
        let files = [
            "SKILL.md",
            "a*b.md",
            "axb.md",
            "Readme.md",
            "x.json",
            "y.yaml",
        ];
        assert_eq!(
            kept(
                &[&file_rules(
                    &["a\\*b.md", "*.{json,yaml}", "readme.md"],
                    None
                )],
                &files
            ),
            ["Readme.md", "SKILL.md", "axb.md"]
        );
    }

    #[test]
    fn skill_md_must_survive_filtering() {
        let filter = FileFilter::new(&[&file_rules(&["*.md"], None)]).unwrap();
        assert_eq!(
            filter.select(&paths(FILES)),
            Err(FilesError::SkillFileDropped {
                reason: Dropped::Excluded(PathPattern::new("*.md").unwrap())
            })
        );
        let filter = FileFilter::new(&[&file_rules(&[], Some(&["references"]))]).unwrap();
        assert_eq!(
            filter.select(&paths(FILES)),
            Err(FilesError::SkillFileDropped {
                reason: Dropped::NotIncluded
            })
        );
        let filter = FileFilter::new(&[]).unwrap();
        assert_eq!(
            filter.select(&paths(&["docs/SKILL.md", "README.md"])),
            Err(FilesError::MissingSkillFile)
        );
    }

    #[test]
    fn invalid_globs_are_reported() {
        let pattern = PathPattern::new("refs/[a").unwrap();
        assert!(pattern_problem(&pattern).is_some());
        assert!(pattern_problem(&PathPattern::new("refs/**").unwrap()).is_none());
        assert!(matches!(
            FileFilter::new(&[&FileRules {
                exclude: vec![pattern],
                include: None,
            }]),
            Err(FilesError::InvalidPattern { .. })
        ));
    }
}
