//! The file listing of a Source at one Revision, and the pure decisions made
//! on it: which entries form a Skill directory, which of them cannot be
//! extracted safely, which license files apply, and which Skill directories
//! lie near a path that no longer exists.

use std::fmt;

use crate::collection::SKILL_FILE;
use crate::safety::{PathError, RelativePath};

/// Number of nearby Skill directories reported when `upstream_path` is not a
/// Skill.
const MAX_CANDIDATES: usize = 5;

/// Kind of a file entry in a Git tree, from its mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Regular file (`100644`).
    File,
    /// Executable regular file (`100755`).
    Executable,
    /// Symbolic link (`120000`); its content is the link target.
    Symlink,
    /// Submodule commit (`160000`).
    Submodule,
    /// Any other mode, as written.
    Other(String),
}

impl Mode {
    fn parse(raw: &str) -> Self {
        match raw {
            "100644" => Self::File,
            "100755" => Self::Executable,
            "120000" => Self::Symlink,
            "160000" => Self::Submodule,
            other => Self::Other(other.to_owned()),
        }
    }

    fn is_regular(&self) -> bool {
        matches!(self, Self::File | Self::Executable)
    }
}

/// One non-directory entry of a recursive tree listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) mode: Mode,
    /// Object identifier of the blob or submodule commit.
    pub(crate) oid: String,
    /// Path from the Source root, as raw bytes: Git does not require UTF-8.
    pub(crate) path: Vec<u8>,
}

/// Parses the output of `git ls-tree -r -z <tree>`: NUL-terminated records
/// of the form `<mode> <type> <object>\t<path>`.
pub(crate) fn parse_listing(output: &[u8]) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for record in output.split(|&byte| byte == 0).filter(|r| !r.is_empty()) {
        let malformed = || {
            format!(
                "unexpected `git ls-tree` record {:?}",
                String::from_utf8_lossy(record)
            )
        };
        let tab = record
            .iter()
            .position(|&byte| byte == b'\t')
            .ok_or_else(malformed)?;
        let (header, path) = (&record[..tab], &record[tab + 1..]);
        let header = std::str::from_utf8(header).map_err(|_| malformed())?;
        let mut fields = header.split(' ');
        let (Some(mode), Some(_kind), Some(oid), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(malformed());
        };
        if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) || path.is_empty() {
            return Err(malformed());
        }
        entries.push(Entry {
            mode: Mode::parse(mode),
            oid: oid.to_owned(),
            path: path.to_vec(),
        });
    }
    Ok(entries)
}

/// Why `upstream_path` names no Skill at a Revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotFoundReason {
    /// Nothing exists at the path.
    Missing,
    /// The path names a file, link, or submodule.
    NotADirectory,
    /// The directory has no `SKILL.md`.
    NoSkillFile,
}

impl fmt::Display for NotFoundReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "the path does not exist",
            Self::NotADirectory => "the path is not a directory",
            Self::NoSkillFile => "the directory has no `SKILL.md`",
        })
    }
}

/// Why an entry of an upstream Skill directory cannot be extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryProblem {
    /// A symbolic link. Skills hold regular files only; `escapes` tells
    /// whether the target lies outside the Skill directory.
    Symlink {
        /// Link target as stored upstream.
        target: String,
        /// Whether the target resolves outside the Skill directory.
        escapes: bool,
    },
    /// The path has a `..` component, an empty or `.` segment, a `.git`
    /// component, a backslash, or a control character.
    InvalidPath(PathError),
    /// The path is not valid UTF-8.
    NonUtf8Path,
    /// A submodule, whose content is not part of the Source.
    Submodule,
    /// A Git file mode that is neither a regular file, a link, nor a
    /// submodule.
    UnsupportedMode(String),
}

impl fmt::Display for EntryProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Symlink {
                target,
                escapes: true,
            } => write!(
                f,
                "symbolic link to {target:?}, which points outside the Skill directory"
            ),
            Self::Symlink {
                target,
                escapes: false,
            } => write!(
                f,
                "symbolic link to {target:?}; Skill directories hold regular files only"
            ),
            Self::InvalidPath(reason) => write!(f, "unsafe path: {reason}"),
            Self::NonUtf8Path => f.write_str("the path is not valid UTF-8"),
            Self::Submodule => f.write_str("submodules are not supported in a Skill directory"),
            Self::UnsupportedMode(mode) => write!(f, "unsupported Git file mode {mode}"),
        }
    }
}

/// An entry of an upstream Skill directory that was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedEntry {
    /// Path relative to the Skill directory, lossily decoded for display.
    pub path: String,
    /// Why it was refused.
    pub problem: EntryProblem,
}

impl fmt::Display for RejectedEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.path, self.problem)
    }
}

/// A regular file of a Skill directory, ready to extract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillFile<'a> {
    /// Path relative to the Skill directory.
    pub(crate) path: RelativePath,
    pub(crate) executable: bool,
    pub(crate) oid: &'a str,
}

/// Result of looking up a Skill directory in a listing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Selection<'a> {
    /// Every entry is a safe regular file, sorted by path.
    Files(Vec<SkillFile<'a>>),
    /// The path is not a Skill directory.
    NotFound(NotFoundReason),
    /// Some entries are unsafe. Symbolic links carry their blob, whose
    /// content is the target, for the caller to describe.
    Unsafe(Vec<Refused<'a>>),
}

/// An unsafe entry before its link target, if any, is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused<'a> {
    pub(crate) path: String,
    pub(crate) kind: Refusal<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal<'a> {
    /// A symbolic link stored in blob `oid`.
    Symlink {
        oid: &'a str,
    },
    Other(EntryProblem),
}

/// The bytes of `path` below `dir`, or `None` when `path` is not inside it.
fn strip_dir<'p>(path: &'p [u8], dir: &RelativePath) -> Option<&'p [u8]> {
    if dir.is_root() {
        return Some(path);
    }
    path.strip_prefix(dir.as_str().as_bytes())?
        .strip_prefix(b"/")
}

/// Selects the entries of the Skill directory `upstream_path`.
pub(crate) fn select_skill<'a>(
    entries: &'a [Entry],
    upstream_path: &RelativePath,
) -> Selection<'a> {
    let inside = entries
        .iter()
        .filter_map(|entry| strip_dir(&entry.path, upstream_path).map(|rel| (entry, rel)))
        .collect::<Vec<_>>();
    if inside.is_empty() {
        let names_entry = entries
            .iter()
            .any(|entry| entry.path == upstream_path.as_str().as_bytes());
        return Selection::NotFound(if names_entry {
            NotFoundReason::NotADirectory
        } else {
            NotFoundReason::Missing
        });
    }
    if !inside.iter().any(|(_, rel)| *rel == SKILL_FILE.as_bytes()) {
        return Selection::NotFound(NotFoundReason::NoSkillFile);
    }

    let mut files = Vec::new();
    let mut refused = Vec::new();
    for (entry, rel) in inside {
        let display = String::from_utf8_lossy(rel).into_owned();
        let refuse = |kind| Refused {
            path: display.clone(),
            kind,
        };
        let path = match std::str::from_utf8(rel) {
            Ok(text) => RelativePath::exact(text),
            Err(_) => {
                refused.push(refuse(Refusal::Other(EntryProblem::NonUtf8Path)));
                continue;
            }
        };
        match (path, &entry.mode) {
            (Err(reason), _) => {
                refused.push(refuse(Refusal::Other(EntryProblem::InvalidPath(reason))));
            }
            (Ok(_), Mode::Symlink) => refused.push(refuse(Refusal::Symlink { oid: &entry.oid })),
            (Ok(_), Mode::Submodule) => {
                refused.push(refuse(Refusal::Other(EntryProblem::Submodule)))
            }
            (Ok(_), Mode::Other(mode)) => refused.push(refuse(Refusal::Other(
                EntryProblem::UnsupportedMode(mode.clone()),
            ))),
            (Ok(path), Mode::File | Mode::Executable) => files.push(SkillFile {
                path,
                executable: entry.mode == Mode::Executable,
                oid: &entry.oid,
            }),
        }
    }
    if refused.is_empty() {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Selection::Files(files)
    } else {
        Selection::Unsafe(refused)
    }
}

/// Whether the link stored at `link` (relative to the Skill directory) with
/// target `target` resolves outside the Skill directory, judged lexically.
pub(crate) fn link_escapes(link: &str, target: &str) -> bool {
    if target.starts_with('/') || target.contains('\\') {
        return true;
    }
    let mut depth = link.split('/').count().saturating_sub(1);
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => match depth.checked_sub(1) {
                Some(parent) => depth = parent,
                None => return true,
            },
            _ => depth += 1,
        }
    }
    false
}

/// Regular files named like a license (`LICENSE*`, `LICENCE*`, `COPYING*`,
/// `UNLICENSE*`) at the Source root, in each directory leading to the Skill,
/// and in the Skill directory itself, from the root down and by name.
pub(crate) fn license_files<'a>(
    entries: &'a [Entry],
    upstream_path: &RelativePath,
) -> Vec<(RelativePath, &'a str)> {
    let mut dirs = vec![String::new()];
    if !upstream_path.is_root() {
        let mut dir = String::new();
        for segment in upstream_path.as_str().split('/') {
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(segment);
            dirs.push(dir.clone());
        }
    }
    let mut found = Vec::new();
    for entry in entries.iter().filter(|entry| entry.mode.is_regular()) {
        let Ok(path) = std::str::from_utf8(&entry.path) else {
            continue;
        };
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let Some(depth) = dirs.iter().position(|candidate| candidate == dir) else {
            continue;
        };
        if is_license_name(name)
            && let Ok(path) = RelativePath::exact(path)
        {
            found.push((depth, path, entry.oid.as_str()));
        }
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    found
        .into_iter()
        .map(|(_, path, oid)| (path, oid))
        .collect()
}

/// Whether a file name is a conventional license file name: a license word,
/// optionally followed by `.`, `-`, or `_` and more text, in any letter case.
fn is_license_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING", "UNLICENSE"]
        .iter()
        .any(|word| match upper.strip_prefix(word) {
            Some(rest) => rest.is_empty() || rest.starts_with(['.', '-', '_']),
            None => false,
        })
}

/// Skill directories (holding a regular `SKILL.md`) closest to `wanted`:
/// same final directory name first, then by edit distance of that name,
/// most leading directories shared with `wanted`, edit distance of the whole
/// path, and path.
pub(crate) fn nearby_skills(entries: &[Entry], wanted: &RelativePath) -> Vec<RelativePath> {
    let wanted_name = last_segment(wanted.as_str());
    let mut scored = entries
        .iter()
        .filter(|entry| entry.mode.is_regular())
        .filter_map(|entry| {
            let path = std::str::from_utf8(&entry.path).ok()?;
            let dir = match path.rsplit_once('/') {
                Some((dir, SKILL_FILE)) => RelativePath::exact(dir).ok()?,
                None if path == SKILL_FILE => RelativePath::new(".").ok()?,
                _ => return None,
            };
            let name = last_segment(dir.as_str());
            let shared = dir
                .as_str()
                .split('/')
                .zip(wanted.as_str().split('/'))
                .take_while(|(a, b)| a == b)
                .count();
            let key = (
                !name.eq_ignore_ascii_case(wanted_name),
                edit_distance(&name.to_lowercase(), &wanted_name.to_lowercase()),
                std::cmp::Reverse(shared),
                edit_distance(dir.as_str(), wanted.as_str()),
            );
            Some((key, dir))
        })
        .collect::<Vec<_>>();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, dir)| dir)
        .collect()
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Levenshtein distance between `a` and `b`, by character.
fn edit_distance(a: &str, b: &str) -> usize {
    let b = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            current.push(substitution.min(deletion).min(insertion));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "45b983be36b73c0788dc9cbcb76cbb80fc7bb057";

    fn entry(mode: &str, path: &str) -> Entry {
        Entry {
            mode: Mode::parse(mode),
            oid: OID.to_owned(),
            path: path.as_bytes().to_vec(),
        }
    }

    fn rel(raw: &str) -> RelativePath {
        RelativePath::new(raw).unwrap()
    }

    fn listing(records: &[&str]) -> Vec<u8> {
        records
            .iter()
            .flat_map(|record| record.bytes().chain([0]))
            .collect()
    }

    #[test]
    fn parses_ls_tree_records() {
        let output = listing(&[
            &format!("100644 blob {OID}\tskills/a/SKILL.md"),
            &format!("100755 blob {OID}\tskills/a/run me.sh"),
            &format!("120000 blob {OID}\tskills/a/link"),
            &format!("160000 commit {OID}\tvendor"),
        ]);
        let entries = parse_listing(&output).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.mode.clone()).collect::<Vec<_>>(),
            [Mode::File, Mode::Executable, Mode::Symlink, Mode::Submodule]
        );
        assert_eq!(entries[1].path, b"skills/a/run me.sh");
        assert_eq!(entries[0].oid, OID);
        assert_eq!(parse_listing(b"").unwrap(), []);
    }

    #[test]
    fn rejects_malformed_records() {
        for record in [
            "100644 blob abc",
            "100644 blob\tx",
            "100644 blob xyz\tx",
            "100644 blob abc extra\tx",
            "100644 blob abc\t",
        ] {
            assert!(parse_listing(&listing(&[record])).is_err(), "{record:?}");
        }
    }

    #[test]
    fn selects_the_files_of_a_skill_directory_sorted() {
        let entries = [
            entry("100644", "README.md"),
            entry("100644", "skills/grill/references/b.md"),
            entry("100755", "skills/grill/SKILL.md"),
            entry("100644", "skills/grill-me/SKILL.md"),
            entry("120000", "skills/other/link"),
        ];
        let Selection::Files(files) = select_skill(&entries, &rel("skills/grill")) else {
            panic!("expected files");
        };
        let paths = files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>();
        assert_eq!(paths, ["SKILL.md", "references/b.md"]);
        assert!(files[0].executable);
        assert!(!files[1].executable);
    }

    #[test]
    fn the_repository_root_can_be_a_skill() {
        let entries = [entry("100644", "SKILL.md"), entry("100644", "docs/x.md")];
        let Selection::Files(files) = select_skill(&entries, &rel(".")) else {
            panic!("expected files");
        };
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn reports_why_a_path_is_not_a_skill() {
        let entries = [
            entry("100644", "skills/a/SKILL.md"),
            entry("100644", "skills/b/README.md"),
            entry("100644", "skills/file"),
        ];
        let cases = [
            ("skills/c", NotFoundReason::Missing),
            ("skill", NotFoundReason::Missing),
            ("skills/file", NotFoundReason::NotADirectory),
            ("skills/b", NotFoundReason::NoSkillFile),
            ("skills", NotFoundReason::NoSkillFile),
        ];
        for (path, reason) in cases {
            assert_eq!(
                select_skill(&entries, &rel(path)),
                Selection::NotFound(reason),
                "{path}"
            );
        }
    }

    #[test]
    fn refuses_links_traversal_submodules_and_odd_modes() {
        let mut entries = vec![
            entry("100644", "s/SKILL.md"),
            entry("120000", "s/link"),
            entry("100644", "s/../escape"),
            entry("100644", "s/a/.git/config"),
            entry("160000", "s/vendor"),
            entry("100664", "s/old"),
        ];
        entries.push(Entry {
            mode: Mode::File,
            oid: OID.to_owned(),
            path: b"s/\xff".to_vec(),
        });
        let Selection::Unsafe(refused) = select_skill(&entries, &rel("s")) else {
            panic!("expected refusal");
        };
        let kinds = refused
            .iter()
            .map(|r| (r.path.as_str(), &r.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                ("link", &Refusal::Symlink { oid: OID }),
                (
                    "../escape",
                    &Refusal::Other(EntryProblem::InvalidPath(PathError::Traversal))
                ),
                (
                    "a/.git/config",
                    &Refusal::Other(EntryProblem::InvalidPath(PathError::GitDir))
                ),
                ("vendor", &Refusal::Other(EntryProblem::Submodule)),
                (
                    "old",
                    &Refusal::Other(EntryProblem::UnsupportedMode("100664".to_owned()))
                ),
                ("\u{fffd}", &Refusal::Other(EntryProblem::NonUtf8Path)),
            ]
        );
    }

    #[test]
    fn judges_link_targets_lexically() {
        let cases = [
            ("link", "SKILL.md", false),
            ("link", "./a/../b", false),
            ("a/link", "../SKILL.md", false),
            ("a/b/link", "../../x", false),
            ("link", "../x", true),
            ("a/link", "../../x", true),
            ("a/link", "b/../../../x", true),
            ("link", "/etc/passwd", true),
            ("link", "..\\x", true),
        ];
        for (link, target, escapes) in cases {
            assert_eq!(link_escapes(link, target), escapes, "{link} -> {target}");
        }
    }

    #[test]
    fn finds_license_files_from_the_root_to_the_skill() {
        let entries = [
            entry("100644", "LICENSE"),
            entry("100644", "COPYING.LESSER"),
            entry("100644", "LICENSE-checker.js"),
            entry("100644", "licenses.txt"),
            entry("100644", "NOTICE"),
            entry("120000", "LICENCE.md"),
            entry("100644", "pstack/License.txt"),
            entry("100644", "pstack/skills/critique/UNLICENSE"),
            entry("100644", "pstack/skills/critique/sub/LICENSE"),
            entry("100644", "other/LICENSE"),
            entry("100644", "LICENSES/MIT.txt"),
        ];
        let found = license_files(&entries, &rel("pstack/skills/critique"))
            .into_iter()
            .map(|(path, _)| path.as_str().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                "COPYING.LESSER",
                "LICENSE",
                "LICENSE-checker.js",
                "pstack/License.txt",
                "pstack/skills/critique/UNLICENSE",
            ]
        );
        let at_root = license_files(&entries, &rel("."));
        assert_eq!(at_root.len(), 3);
    }

    #[test]
    fn license_names_need_a_separator_after_the_word() {
        for name in [
            "LICENSE",
            "license.md",
            "Licence",
            "COPYING",
            "LICENSE_MIT",
            "UNLICENSE",
        ] {
            assert!(is_license_name(name), "{name}");
        }
        for name in [
            "LICENSES",
            "licensed.txt",
            "MY-LICENSE",
            "COPYINGS",
            "NOTICE",
        ] {
            assert!(!is_license_name(name), "{name}");
        }
    }

    #[test]
    fn ranks_nearby_skill_directories() {
        let entries = [
            entry("100644", "SKILL.md"),
            entry("100644", "skills/engineering/grill-me/SKILL.md"),
            entry("100644", "skills/grilling/SKILL.md"),
            entry("100644", "skills/review/SKILL.md"),
            entry("100644", "skills/review/notes/SKILL.md.bak"),
            entry("120000", "skills/linked/SKILL.md"),
            entry("100644", "archive/grill-me/SKILL.md"),
            entry("100644", "zzzzzzzzzzzz/SKILL.md"),
        ];
        let found = nearby_skills(&entries, &rel("skills/grill-me"))
            .iter()
            .map(|path| path.as_str().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                "skills/engineering/grill-me",
                "archive/grill-me",
                "skills/grilling",
                "skills/review",
                ".",
            ]
        );
        assert_eq!(
            nearby_skills(&[], &rel("skills/a")),
            Vec::<RelativePath>::new()
        );
    }

    #[test]
    fn edit_distance_counts_single_character_edits() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("grill", "grill"), 0);
        assert_eq!(edit_distance("grill", "grills"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }
}
