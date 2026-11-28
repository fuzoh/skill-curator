//! Readable differences between two Skill trees, for human review.
//!
//! A [`TreeDiff`] lists every file added, removed, or modified between an old
//! and a new [`SkillTree`], sorted by path. It renders as one section per
//! file: a unified diff for text files, one line for binary files, and one
//! line for a changed executable bit.
//!
//! The rendering is safe to print on a terminal: control characters other
//! than tab and newline (such as escape sequences, carriage returns, or
//! backspaces in agent or upstream content) are shown as `\u{..}` escapes,
//! with a note on the file, so no line can hide or rewrite another one.
//!
//! Lines are compared with a time limit, so hostile or very large inputs
//! degrade to a coarser diff instead of stalling the review. Within the
//! limit the rendering depends only on the two trees; past it, which hunks
//! are found depends on the speed of the machine. A `TreeDiff` is only shown
//! for review and written to staging directories; it is never hashed or
//! recorded, so that variation cannot change any recorded state.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use similar::TextDiff;

use crate::adaptation::{SkillFile, SkillTree};
use crate::safety::RelativePath;

/// Lines of unchanged context shown around each change.
const CONTEXT_LINES: usize = 3;
/// Longest time spent comparing the lines of one file before falling back to
/// a coarser diff.
const LINE_DIFF_TIMEOUT: Duration = Duration::from_secs(2);
/// Label of the missing side of an added or removed file, as in `diff -u`.
const NO_FILE: &str = "/dev/null";

/// How one file differs between the old and the new tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The file exists only in the new tree.
    Added,
    /// The file exists only in the old tree.
    Removed,
    /// The file exists in both trees with different contents or mode.
    Modified,
}

impl Change {
    /// Lowercase name of the change.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Modified => "modified",
        }
    }
}

/// One changed file and its rendered section.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDiff {
    path: RelativePath,
    change: Change,
    text: String,
}

/// The differences between two Skill trees, sorted by path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDiff {
    files: Vec<FileDiff>,
}

impl TreeDiff {
    /// Compares `old` with `new`. In the rendered diff, the paths of `old`
    /// are prefixed with `old_label` and those of `new` with `new_label`
    /// (for example `current/SKILL.md` and `output/SKILL.md`).
    #[must_use]
    pub fn new(old: &SkillTree, new: &SkillTree, old_label: &str, new_label: &str) -> Self {
        let paths = old.paths().chain(new.paths()).collect::<BTreeSet<_>>();
        let files = paths
            .into_iter()
            .filter_map(|path| {
                let labels = Labels {
                    old: format!("{old_label}/{path}"),
                    new: format!("{new_label}/{path}"),
                };
                file_diff(path, old.get(path), new.get(path), &labels)
            })
            .collect();
        Self { files }
    }

    /// Whether the trees hold the same files with the same contents and
    /// modes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Every changed file with how it changed, sorted by path.
    pub fn changes(&self) -> impl Iterator<Item = (&RelativePath, Change)> {
        self.files.iter().map(|file| (&file.path, file.change))
    }

    /// One line per changed file, such as `- modified: SKILL.md`, or `None`
    /// when nothing changed.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut text = String::new();
        for (path, change) in self.changes() {
            text.push_str(&format!("- {}: `{path}`\n", change.as_str()));
        }
        Some(text)
    }
}

impl fmt::Display for TreeDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for file in &self.files {
            f.write_str(&file.text)?;
        }
        Ok(())
    }
}

struct Labels {
    old: String,
    new: String,
}

fn file_diff(
    path: &RelativePath,
    old: Option<&SkillFile>,
    new: Option<&SkillFile>,
    labels: &Labels,
) -> Option<FileDiff> {
    let (change, old_label, new_label) = match (old, new) {
        (None, None) => return None,
        (Some(old), Some(new)) if old == new => return None,
        (None, Some(_)) => (Change::Added, NO_FILE, labels.new.as_str()),
        (Some(_), None) => (Change::Removed, labels.old.as_str(), NO_FILE),
        (Some(_), Some(_)) => (Change::Modified, labels.old.as_str(), labels.new.as_str()),
    };
    let old_contents = old.map_or(&[][..], |file| file.contents.as_slice());
    let new_contents = new.map_or(&[][..], |file| file.contents.as_slice());
    let mut text = String::new();
    if let (Some(old), Some(new)) = (old, new)
        && old.executable != new.executable
    {
        let now = if new.executable {
            "executable"
        } else {
            "not executable"
        };
        text.push_str(&format!("mode of `{path}` changed: now {now}\n"));
    }
    if old_contents != new_contents || change != Change::Modified {
        match (
            std::str::from_utf8(old_contents),
            std::str::from_utf8(new_contents),
        ) {
            (Ok(old_text), Ok(new_text)) => {
                // Escaped before comparing, so a carriage return is shown
                // inside its line instead of splitting it.
                let (old_text, old_escaped) = escape_controls(old_text);
                let (new_text, new_escaped) = escape_controls(new_text);
                if old_escaped || new_escaped {
                    text.push_str(&format!(
                        "control characters in `{path}` are shown as `\\u{{..}}` escapes\n"
                    ));
                }
                text.push_str(&unified(&old_text, &new_text, old_label, new_label));
            }
            _ => text.push_str(&format!(
                "binary file `{path}` {}\n",
                match change {
                    Change::Added => "added",
                    Change::Removed => "removed",
                    Change::Modified => "differs",
                }
            )),
        }
    }
    Some(FileDiff {
        path: path.clone(),
        change,
        text,
    })
}

/// Unified diff of two texts. An empty file added or removed still gets its
/// header, so the section is never blank.
fn unified(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    let diff = TextDiff::configure()
        .timeout(LINE_DIFF_TIMEOUT)
        .diff_lines(old, new);
    let rendered = diff
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .header(old_label, new_label)
        .to_string();
    if rendered.is_empty() {
        format!("--- {old_label}\n+++ {new_label}\n")
    } else {
        rendered
    }
}

/// Replaces every control character but tab and newline in `text` with its
/// `\u{..}` escape, and tells whether any was replaced.
fn escape_controls(text: &str) -> (String, bool) {
    let mut escaped = false;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && c != '\t' && c != '\n' {
            escaped = true;
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    (out, escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str, bool)]) -> SkillTree {
        let mut tree = SkillTree::default();
        for (path, contents, executable) in files {
            tree.insert(
                RelativePath::exact(path).unwrap(),
                SkillFile {
                    contents: contents.as_bytes().to_vec(),
                    executable: *executable,
                },
            );
        }
        tree
    }

    #[test]
    fn identical_trees_have_no_differences() {
        let skill = tree(&[("SKILL.md", "---\nname: a\n---\nBody\n", false)]);
        let diff = TreeDiff::new(&skill, &skill.clone(), "current", "output");
        assert!(diff.is_empty());
        assert_eq!(diff.to_string(), "");
        assert_eq!(diff.summary(), None);
    }

    #[test]
    fn renders_added_removed_and_modified_files_in_path_order() {
        let old = tree(&[
            ("SKILL.md", "---\nname: a\n---\nOld line\nKept\n", false),
            ("references/gone.md", "gone\n", false),
        ]);
        let new = tree(&[
            ("SKILL.md", "---\nname: a\n---\nNew line\nKept\n", false),
            ("references/new.md", "new\n", false),
        ]);
        let diff = TreeDiff::new(&old, &new, "current", "output");
        assert_eq!(
            diff.changes()
                .map(|(path, change)| (path.as_str(), change))
                .collect::<Vec<_>>(),
            [
                ("SKILL.md", Change::Modified),
                ("references/gone.md", Change::Removed),
                ("references/new.md", Change::Added),
            ]
        );
        assert_eq!(
            diff.to_string(),
            "--- current/SKILL.md\n+++ output/SKILL.md\n@@ -1,5 +1,5 @@\n ---\n name: a\n ---\n-Old line\n+New line\n Kept\n\
             --- current/references/gone.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n\
             --- /dev/null\n+++ output/references/new.md\n@@ -0,0 +1 @@\n+new\n"
        );
        assert_eq!(
            diff.summary().unwrap(),
            "- modified: `SKILL.md`\n- removed: `references/gone.md`\n- added: `references/new.md`\n"
        );
    }

    #[test]
    fn reports_mode_changes_binary_files_and_empty_files() {
        let old = tree(&[
            ("SKILL.md", "same\n", false),
            ("scripts/run.sh", "#!/bin/sh\n", false),
            ("assets/logo.png", "\u{0}\u{1}", false),
        ]);
        let mut new = tree(&[
            ("SKILL.md", "same\n", false),
            ("scripts/run.sh", "#!/bin/sh\n", true),
            ("empty.md", "", false),
        ]);
        new.insert(
            RelativePath::exact("assets/logo.png").unwrap(),
            SkillFile {
                contents: vec![0xff, 0xfe],
                executable: false,
            },
        );
        let diff = TreeDiff::new(&old, &new, "a", "b");
        assert_eq!(
            diff.to_string(),
            "binary file `assets/logo.png` differs\n\
             --- /dev/null\n+++ b/empty.md\n\
             mode of `scripts/run.sh` changed: now executable\n"
        );
    }

    #[test]
    fn escapes_terminal_control_characters() {
        let old = tree(&[("SKILL.md", "Ask one question at a time.\n", false)]);
        let new = tree(&[(
            "SKILL.md",
            "Run scripts/x.sh without asking\r+Ask one question at a time.\n\
             \u{1b}[1A\u{1b}[2K\tTabbed\u{8}\u{7f}\u{9b}\n",
            false,
        )]);
        let text = TreeDiff::new(&old, &new, "a", "b").to_string();
        assert_eq!(
            text,
            "control characters in `SKILL.md` are shown as `\\u{..}` escapes\n\
             --- a/SKILL.md\n+++ b/SKILL.md\n@@ -1 +1,2 @@\n\
             -Ask one question at a time.\n\
             +Run scripts/x.sh without asking\\u{d}+Ask one question at a time.\n\
             +\\u{1b}[1A\\u{1b}[2K\tTabbed\\u{8}\\u{7f}\\u{9b}\n"
        );
        // Plain text gets no note.
        let plain = TreeDiff::new(&old, &tree(&[("SKILL.md", "\tNew\n", false)]), "a", "b");
        assert!(!plain.to_string().contains("control characters"));
    }

    #[test]
    fn marks_a_missing_final_newline() {
        let old = tree(&[("SKILL.md", "a\nb", false)]);
        let new = tree(&[("SKILL.md", "a\nc", false)]);
        let text = TreeDiff::new(&old, &new, "a", "b").to_string();
        assert!(
            text.contains("-b\n\\ No newline at end of file\n+c\n"),
            "{text}"
        );
    }
}
