//! Assembly of a global instruction file from Fragments: the managed header,
//! then each Fragment's text in declared order. There is no templating; the
//! Fragments are copied byte for byte.

use std::fs;
use std::path::Path;

use crate::collection::{CollectionError, Entry, inspect};
use crate::safety::RelativePath;

/// First line of every assembled global instruction file.
pub const MANAGED_HEADER: &str = "<!-- Managed by skill-curator -->";

/// Assembles the global instruction file from `fragments`, read from the
/// Collection at `root`.
///
/// The result is [`MANAGED_HEADER`], a blank line, then each Fragment,
/// separated by a blank line. A Fragment that does not end with a line
/// break gets one, so the next Fragment starts on its own line.
///
/// # Errors
///
/// Returns a [`CollectionError`] when a Fragment is not a regular file of
/// the Collection reached without symbolic links, or cannot be read.
pub fn assemble(root: &Path, fragments: &[RelativePath]) -> Result<Vec<u8>, CollectionError> {
    let mut text = format!("{MANAGED_HEADER}\n").into_bytes();
    for fragment in fragments {
        let path = fragment.join_to(root);
        let segments = fragment.as_str().split('/').collect::<Vec<_>>();
        let reason = match inspect(root, &segments)? {
            Entry::File => None,
            Entry::Missing => Some("it does not exist"),
            Entry::Symlink(_) => Some("it is reached through a symbolic link"),
            Entry::Dir | Entry::Other => Some("it is not a regular file"),
        };
        if let Some(reason) = reason {
            return Err(CollectionError::Read {
                path,
                source: std::io::Error::other(format!(
                    "Fragment `{fragment}` cannot be assembled: {reason}"
                )),
            });
        }
        let contents = fs::read(&path).map_err(|source| CollectionError::Read { path, source })?;
        text.push(b'\n');
        text.extend_from_slice(&contents);
        if !contents.ends_with(b"\n") {
            text.push(b'\n');
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    fn path(raw: &str) -> RelativePath {
        RelativePath::new(raw).unwrap()
    }

    fn collection() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("fragments")).unwrap();
        fs::write(
            dir.path().join("fragments/git.md"),
            "# Git\n\nUse Conventional Commits.\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("fragments/uv.md"),
            "Run Python with `uv run`.",
        )
        .unwrap();
        dir
    }

    #[test]
    fn concatenates_fragments_in_declared_order_after_the_header() {
        let dir = collection();
        let text = assemble(
            dir.path(),
            &[path("fragments/uv.md"), path("fragments/git.md")],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "<!-- Managed by skill-curator -->\n\nRun Python with `uv run`.\n\n# Git\n\nUse Conventional Commits.\n"
        );
        let text = assemble(
            dir.path(),
            &[path("fragments/git.md"), path("fragments/uv.md")],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "<!-- Managed by skill-curator -->\n\n# Git\n\nUse Conventional Commits.\n\nRun Python with `uv run`.\n"
        );
        assert_eq!(
            assemble(dir.path(), &[]).unwrap(),
            b"<!-- Managed by skill-curator -->\n"
        );
    }

    #[test]
    fn fragments_are_copied_byte_for_byte_without_templating() {
        let dir = collection();
        let raw = "{{ name }} ${HOME} <%= x %>\r\nkeep\r\n";
        fs::write(dir.path().join("fragments/raw.md"), raw).unwrap();
        let text = assemble(dir.path(), &[path("fragments/raw.md")]).unwrap();
        assert!(text.ends_with(raw.as_bytes()));
    }

    #[test]
    fn refuses_missing_linked_or_non_regular_fragments() {
        let dir = collection();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.md"), "secret").unwrap();
        symlink(
            outside.path().join("secret.md"),
            dir.path().join("fragments/link.md"),
        )
        .unwrap();
        fs::create_dir(dir.path().join("fragments/dir.md")).unwrap();
        for (fragment, reason) in [
            ("fragments/missing.md", "does not exist"),
            ("fragments/link.md", "symbolic link"),
            ("fragments/dir.md", "not a regular file"),
        ] {
            let err = assemble(dir.path(), &[path(fragment)])
                .unwrap_err()
                .to_string();
            assert!(err.contains(reason), "{fragment}: {err}");
        }
        fs::remove_dir_all(dir.path().join("fragments")).unwrap();
        symlink(outside.path(), dir.path().join("fragments")).unwrap();
        fs::write(outside.path().join("git.md"), "git").unwrap();
        let err = assemble(dir.path(), &[path("fragments/git.md")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("symbolic link"), "{err}");
    }
}
