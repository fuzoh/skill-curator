//! Validation of names and paths that come from user- or upstream-supplied
//! text before they reach the filesystem.
//!
//! Every type here is a checked newtype: holding a value proves it passed
//! validation, so later code can join it onto a root directory without
//! re-checking for path traversal.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collection::SKILL_FILE;

/// Maximum length of a [`Name`], matching the Agent Skills limit for Skill
/// names.
const MAX_NAME_LEN: usize = 64;

/// Why a name or path was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathError {
    /// The value is empty.
    Empty,
    /// The value contains a NUL byte or another control character.
    ControlCharacter,
    /// A relative path or pattern starts at the filesystem root.
    Absolute,
    /// The value contains a `..` component that could escape its root.
    Traversal,
    /// The value contains a backslash, which is not a portable separator.
    Backslash,
    /// An installation path is neither absolute nor relative to the home
    /// directory (`~/`).
    NotAnchored,
    /// An installation path names the filesystem root or the home directory
    /// itself, where managed files would mix with unrelated ones.
    RootOrHome,
    /// A directory that must be absolute is relative.
    NotAbsolute,
    /// A name contains characters outside `a-z`, `0-9`, `-`, and `_`, or does
    /// not start with a letter or digit.
    InvalidName,
    /// A name is longer than 64 characters.
    NameTooLong,
    /// A path read from an upstream tree has an empty or `.` segment, so it
    /// does not name an entry exactly.
    DotSegment,
    /// A path read from an upstream tree has a `.git` component, which Git
    /// tools would read as repository metadata.
    GitDir,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::Empty => "it is empty",
            Self::ControlCharacter => "it contains a control character",
            Self::Absolute => "it must be relative, not start with `/`",
            Self::Traversal => "it must not contain a `..` component",
            Self::Backslash => "it must use `/` as separator, not `\\`",
            Self::NotAnchored => "it must be absolute or start with `~/`",
            Self::RootOrHome => {
                "it must name a path below `/` or `~/`, not the filesystem root or the home directory itself"
            }
            Self::NotAbsolute => "it must be an absolute path",
            Self::InvalidName => {
                "it must start with a lowercase letter or digit and contain only `a-z`, `0-9`, `-`, and `_`"
            }
            Self::NameTooLong => "it must be at most 64 characters long",
            Self::DotSegment => "it must not contain empty or `.` segments",
            Self::GitDir => "it must not contain a `.git` component",
        };
        f.write_str(reason)
    }
}

impl std::error::Error for PathError {}

/// A rejected value, with what it was meant to be and why it was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidValue {
    value: String,
    what: &'static str,
    reason: PathError,
}

impl InvalidValue {
    fn new(value: String, what: &'static str, reason: PathError) -> Self {
        Self {
            value,
            what,
            reason,
        }
    }

    /// Why the value was rejected.
    #[must_use]
    pub fn reason(&self) -> &PathError {
        &self.reason
    }
}

impl fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a valid {}: {}",
            self.value, self.what, self.reason
        )
    }
}

impl std::error::Error for InvalidValue {}

/// Rejects empty values and control characters shared by every check.
fn check_common(raw: &str) -> Result<(), PathError> {
    if raw.is_empty() {
        return Err(PathError::Empty);
    }
    if raw.chars().any(char::is_control) {
        return Err(PathError::ControlCharacter);
    }
    Ok(())
}

/// Splits a relative path on `/`, dropping empty and `.` segments, and rejects
/// `..` segments and absolute paths.
fn relative_segments(raw: &str) -> Result<Vec<&str>, PathError> {
    check_common(raw)?;
    if raw.contains('\\') {
        return Err(PathError::Backslash);
    }
    if raw.starts_with('/') {
        return Err(PathError::Absolute);
    }
    let mut segments = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(PathError::Traversal),
            _ => segments.push(segment),
        }
    }
    Ok(segments)
}

/// Identifier of a Source, Skill, preset, Composition input, or Destination.
///
/// Names become directory names (for example `skills/<name>/` in the
/// Collection and in every Destination), so they are restricted to a single
/// safe path component: a lowercase letter or digit followed by up to 63
/// lowercase letters, digits, `-`, or `_`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Name(String);

impl Name {
    /// Validates `raw` as a name.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] when `raw` is empty, too long, or contains a
    /// character that is not allowed.
    pub fn new(raw: &str) -> Result<Self, PathError> {
        check_common(raw)?;
        if raw.len() > MAX_NAME_LEN {
            return Err(PathError::NameTooLong);
        }
        let mut chars = raw.chars();
        let starts_well = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let rest_ok =
            chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        if !(starts_well && rest_ok) {
            return Err(PathError::InvalidName);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Name {
    type Error = InvalidValue;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).map_err(|reason| InvalidValue::new(raw, "name", reason))
    }
}

impl From<Name> for String {
    fn from(name: Name) -> Self {
        name.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `/`-separated path that stays inside the directory it is joined to.
///
/// It is never absolute and never contains `..`. Redundant `.` segments and
/// repeated or trailing slashes are dropped, so `./skills//grill/` is stored
/// as `skills/grill`. A path made only of `.` segments denotes the root
/// itself and is stored as `.`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RelativePath(String);

impl RelativePath {
    /// Validates and normalises `raw`.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] when `raw` is empty, absolute, contains a `..`
    /// component, a backslash, or a control character.
    pub fn new(raw: &str) -> Result<Self, PathError> {
        let segments = relative_segments(raw)?;
        if segments.is_empty() {
            return Ok(Self(".".to_owned()));
        }
        Ok(Self(segments.join("/")))
    }

    /// Validates a path read from an upstream Git tree, which must name an
    /// entry exactly: on top of the checks of [`RelativePath::new`], it
    /// rejects empty and `.` segments (including a trailing `/`) instead of
    /// normalising them, and any `.git` component in any letter case.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] when `raw` fails [`RelativePath::new`], has an
    /// empty or `.` segment, or a `.git` component.
    pub fn exact(raw: &str) -> Result<Self, PathError> {
        relative_segments(raw)?;
        for segment in raw.split('/') {
            if segment.is_empty() || segment == "." {
                return Err(PathError::DotSegment);
            }
            if segment.eq_ignore_ascii_case(".git") {
                return Err(PathError::GitDir);
            }
        }
        Ok(Self(raw.to_owned()))
    }

    /// The path of a Skill's `SKILL.md`, relative to the Skill directory.
    #[must_use]
    pub fn skill_file() -> Self {
        Self(SKILL_FILE.to_owned())
    }

    /// The normalised path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the path denotes the root it is joined to.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0 == "."
    }

    /// Whether the first segment of the path is `dir`.
    #[must_use]
    pub fn starts_with_dir(&self, dir: &str) -> bool {
        self.0.split('/').next() == Some(dir) && self.0 != dir
    }

    /// Joins the path onto `root`. The result never leaves `root`.
    #[must_use]
    pub fn join_to(&self, root: &Path) -> PathBuf {
        if self.is_root() {
            return root.to_path_buf();
        }
        root.join(&self.0)
    }
}

impl TryFrom<String> for RelativePath {
    type Error = InvalidValue;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).map_err(|reason| InvalidValue::new(raw, "relative path", reason))
    }
}

impl From<RelativePath> for String {
    fn from(path: RelativePath) -> Self {
        path.0
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A relative path glob (D2 `exclude` or `include` entry), kept verbatim.
///
/// Glob syntax is interpreted by the file filter; this type only guarantees
/// the pattern cannot name anything outside the Skill directory: it is not
/// absolute and has no `..` segment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PathPattern(String);

impl PathPattern {
    /// Validates `raw` as a relative pattern.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] when `raw` is empty, absolute, contains a `..`
    /// segment, or a control character.
    pub fn new(raw: &str) -> Result<Self, PathError> {
        check_common(raw)?;
        if raw.starts_with('/') {
            return Err(PathError::Absolute);
        }
        if raw.split('/').any(|segment| segment == "..") {
            return Err(PathError::Traversal);
        }
        Ok(Self(raw.to_owned()))
    }

    /// The pattern as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PathPattern {
    type Error = InvalidValue;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).map_err(|reason| InvalidValue::new(raw, "path pattern", reason))
    }
}

impl From<PathPattern> for String {
    fn from(pattern: PathPattern) -> Self {
        pattern.0
    }
}

impl fmt::Display for PathPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An installation path outside the Collection: absolute (`/opt/skills`) or
/// relative to the home directory (`~/.claude/skills`).
///
/// It never contains `..`, so a Destination cannot be redirected through a
/// parent directory, and it is never `/` or `~` alone, so a Destination
/// cannot claim the filesystem root or the whole home directory. Normalised
/// like [`RelativePath`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstallPath(String);

impl InstallPath {
    /// Validates and normalises `raw`.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] when `raw` is empty, neither absolute nor
    /// `~/`-relative, names `/` or `~` itself, or contains a `..` component,
    /// a backslash, or a control character.
    pub fn new(raw: &str) -> Result<Self, PathError> {
        check_common(raw)?;
        let (anchor, rest) = if let Some(rest) = raw.strip_prefix("~/") {
            ("~/", rest)
        } else if let Some(rest) = raw.strip_prefix('/') {
            ("/", rest)
        } else if raw == "~" {
            ("~/", "")
        } else {
            return Err(PathError::NotAnchored);
        };
        let rest = rest.trim_start_matches('/');
        let segments = if rest.is_empty() {
            Vec::new()
        } else {
            relative_segments(rest)?
        };
        if segments.is_empty() {
            return Err(PathError::RootOrHome);
        }
        Ok(Self(format!("{anchor}{}", segments.join("/"))))
    }

    /// The normalised path, with `~` unexpanded.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path with a leading `~` replaced by `home`. The result is always
    /// absolute.
    #[must_use]
    pub fn expand(&self, home: &HomeDir) -> PathBuf {
        match self.0.strip_prefix("~/") {
            Some(rest) => home.as_path().join(rest),
            None => PathBuf::from(&self.0),
        }
    }
}

impl TryFrom<String> for InstallPath {
    type Error = InvalidValue;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw).map_err(|reason| InvalidValue::new(raw, "installation path", reason))
    }
}

impl From<InstallPath> for String {
    fn from(path: InstallPath) -> Self {
        path.0
    }
}

impl fmt::Display for InstallPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The home directory that a leading `~` in an [`InstallPath`] stands for.
///
/// It is absolute and has no `..` component, so every expanded installation
/// path is absolute too and never resolves against the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeDir(PathBuf);

impl HomeDir {
    /// Validates `path` as a home directory.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] when `path` is empty, relative, or contains a
    /// `..` component.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, PathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(PathError::Empty);
        }
        if !path.is_absolute() {
            return Err(PathError::NotAbsolute);
        }
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(PathError::Traversal);
        }
        Ok(Self(path))
    }

    /// The home directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_accept_lowercase_identifiers() {
        for raw in ["grill-me", "a", "0x", "pstack_review", &"a".repeat(64)] {
            assert_eq!(Name::new(raw).unwrap().as_str(), raw);
        }
    }

    #[test]
    fn names_reject_unsafe_or_unportable_text() {
        let cases = [
            ("", PathError::Empty),
            ("..", PathError::InvalidName),
            (".hidden", PathError::InvalidName),
            ("-flag", PathError::InvalidName),
            ("_x", PathError::InvalidName),
            ("a/b", PathError::InvalidName),
            ("Grill", PathError::InvalidName),
            ("grill me", PathError::InvalidName),
            ("é", PathError::InvalidName),
            ("a\0b", PathError::ControlCharacter),
        ];
        for (raw, expected) in cases {
            assert_eq!(Name::new(raw), Err(expected), "{raw:?}");
        }
        assert_eq!(Name::new(&"a".repeat(65)), Err(PathError::NameTooLong));
    }

    #[test]
    fn relative_paths_are_normalised() {
        let cases = [
            ("skills/grill", "skills/grill"),
            ("./skills//grill/", "skills/grill"),
            ("a/./b", "a/b"),
            (".", "."),
            ("./", "."),
            ("a..b/c", "a..b/c"),
            ("...", "..."),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                RelativePath::new(raw).unwrap().as_str(),
                expected,
                "{raw:?}"
            );
        }
    }

    #[test]
    fn relative_paths_reject_escapes() {
        let cases = [
            ("", PathError::Empty),
            ("/etc/passwd", PathError::Absolute),
            ("..", PathError::Traversal),
            ("../x", PathError::Traversal),
            ("a/../../x", PathError::Traversal),
            ("a/..", PathError::Traversal),
            ("a\\..\\x", PathError::Backslash),
            ("a\nb", PathError::ControlCharacter),
        ];
        for (raw, expected) in cases {
            assert_eq!(RelativePath::new(raw), Err(expected), "{raw:?}");
        }
    }

    #[test]
    fn exact_paths_name_upstream_entries_verbatim() {
        for raw in [
            "SKILL.md",
            "skills/grill-me/SKILL.md",
            "a..b/c",
            ".github/x",
            ".gitignore",
        ] {
            assert_eq!(RelativePath::exact(raw).unwrap().as_str(), raw);
        }
        let cases = [
            ("", PathError::Empty),
            ("/etc/passwd", PathError::Absolute),
            ("..", PathError::Traversal),
            ("a/../b", PathError::Traversal),
            ("a\\b", PathError::Backslash),
            ("a\tb", PathError::ControlCharacter),
            (".", PathError::DotSegment),
            ("a/./b", PathError::DotSegment),
            ("a//b", PathError::DotSegment),
            ("a/", PathError::DotSegment),
            (".git", PathError::GitDir),
            ("a/.git/config", PathError::GitDir),
            ("a/.GIT/hooks", PathError::GitDir),
        ];
        for (raw, expected) in cases {
            assert_eq!(RelativePath::exact(raw), Err(expected), "{raw:?}");
        }
    }

    #[test]
    fn relative_paths_join_inside_their_root() {
        let root = Path::new("/collection");
        let intent = RelativePath::new("intents/review.md").unwrap();
        assert_eq!(
            intent.join_to(root),
            Path::new("/collection/intents/review.md")
        );
        assert_eq!(RelativePath::new(".").unwrap().join_to(root), root);
    }

    #[test]
    fn relative_paths_know_their_first_directory() {
        let intent = RelativePath::new("intents/review.md").unwrap();
        assert!(intent.starts_with_dir("intents"));
        assert!(!intent.starts_with_dir("fragments"));
        assert!(
            !RelativePath::new("intents")
                .unwrap()
                .starts_with_dir("intents")
        );
        assert!(
            !RelativePath::new("intents-old/x.md")
                .unwrap()
                .starts_with_dir("intents")
        );
        assert!(RelativePath::new(".").unwrap().is_root());
        assert!(!intent.is_root());
    }

    #[test]
    fn patterns_keep_globs_but_reject_escapes() {
        for raw in ["assets/**", "*.png", "**/automations", "docs/[ab].md"] {
            assert_eq!(PathPattern::new(raw).unwrap().as_str(), raw);
        }
        let cases = [
            ("", PathError::Empty),
            ("/abs/**", PathError::Absolute),
            ("../**", PathError::Traversal),
            ("a/../b", PathError::Traversal),
            ("**/..", PathError::Traversal),
            ("a\tb", PathError::ControlCharacter),
        ];
        for (raw, expected) in cases {
            assert_eq!(PathPattern::new(raw), Err(expected), "{raw:?}");
        }
    }

    #[test]
    fn install_paths_are_anchored_and_normalised() {
        let cases = [
            ("~/.claude/skills", "~/.claude/skills"),
            ("~/.claude//skills/", "~/.claude/skills"),
            ("/opt/skills", "/opt/skills"),
            ("//opt/./skills", "/opt/skills"),
        ];
        for (raw, expected) in cases {
            assert_eq!(InstallPath::new(raw).unwrap().as_str(), expected, "{raw:?}");
        }
    }

    #[test]
    fn install_paths_reject_relative_and_traversal() {
        let cases = [
            ("", PathError::Empty),
            ("skills", PathError::NotAnchored),
            ("./skills", PathError::NotAnchored),
            ("~user/skills", PathError::NotAnchored),
            ("~/../etc", PathError::Traversal),
            ("/opt/../etc", PathError::Traversal),
            ("~/a\\b", PathError::Backslash),
            ("/a\0", PathError::ControlCharacter),
            ("/", PathError::RootOrHome),
            ("//./", PathError::RootOrHome),
            ("~", PathError::RootOrHome),
            ("~/", PathError::RootOrHome),
            ("~/./", PathError::RootOrHome),
        ];
        for (raw, expected) in cases {
            assert_eq!(InstallPath::new(raw), Err(expected), "{raw:?}");
        }
    }

    #[test]
    fn install_paths_expand_the_home_directory_only() {
        let home = HomeDir::new("/home/user").unwrap();
        let cases = [
            ("~/.claude/skills", "/home/user/.claude/skills"),
            ("~/.claude/CLAUDE.md", "/home/user/.claude/CLAUDE.md"),
            ("/opt/skills", "/opt/skills"),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                InstallPath::new(raw).unwrap().expand(&home),
                Path::new(expected)
            );
        }
    }

    #[test]
    fn home_directories_are_absolute() {
        assert_eq!(
            HomeDir::new("/home/user").unwrap().as_path(),
            Path::new("/home/user")
        );
        let cases = [
            ("", PathError::Empty),
            ("home/user", PathError::NotAbsolute),
            ("~", PathError::NotAbsolute),
            ("/home/../etc", PathError::Traversal),
        ];
        for (raw, expected) in cases {
            assert_eq!(HomeDir::new(raw), Err(expected), "{raw:?}");
        }
    }

    #[test]
    fn conversion_errors_name_the_value_and_its_kind() {
        let err = RelativePath::try_from("../x".to_owned()).unwrap_err();
        assert_eq!(err.reason(), &PathError::Traversal);
        assert_eq!(
            err.to_string(),
            "\"../x\" is not a valid relative path: it must not contain a `..` component"
        );
        let err = Name::try_from("Bad".to_owned()).unwrap_err();
        assert!(err.to_string().starts_with("\"Bad\" is not a valid name: "));
        let err = PathPattern::try_from("/x".to_owned()).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("\"/x\" is not a valid path pattern: ")
        );
        let err = InstallPath::try_from("x".to_owned()).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("\"x\" is not a valid installation path: ")
        );
    }

    #[test]
    fn errors_explain_the_rule() {
        assert_eq!(
            PathError::Traversal.to_string(),
            "it must not contain a `..` component"
        );
        assert_eq!(
            PathError::NotAnchored.to_string(),
            "it must be absolute or start with `~/`"
        );
    }
}
