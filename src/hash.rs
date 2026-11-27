//! Deterministic integrity hashes recorded in `lock.toml`.
//!
//! - [`tree_digest`] hashes a materialised Skill directory: its sorted
//!   relative file paths and their contents. File creation order, timestamps,
//!   permissions, and empty directories do not affect the result.
//! - [`inputs_digest`] hashes the normalised inputs of an Adaptation or
//!   Composition: a structured value (the declaration and its D1/D2 rules)
//!   plus the bytes of the written intent, if any.
//!
//! Both encodings are length-prefixed and domain-separated, so no two
//! different inputs share an encoding, and neither depends on how the TOML
//! library formats or orders text.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Prefix of every digest string, naming the algorithm.
const SHA256_PREFIX: &str = "sha256:";
/// Number of hexadecimal characters in a SHA-256 digest.
const SHA256_HEX_LEN: usize = 64;

/// Domain separator for materialised Skill directory digests.
const TREE_DOMAIN: &[u8] = b"skill-curator/tree/v1\0";
/// Domain separator for Adaptation and Composition input digests.
const INPUTS_DOMAIN: &[u8] = b"skill-curator/inputs/v1\0";

/// A SHA-256 digest written as `sha256:<64 lowercase hex digits>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Digest(String);

impl Digest {
    /// Parses a digest string.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidDigest`] when `raw` is not `sha256:` followed by 64
    /// lowercase hexadecimal digits.
    pub fn parse(raw: &str) -> Result<Self, InvalidDigest> {
        let hex = raw.strip_prefix(SHA256_PREFIX).ok_or(InvalidDigest)?;
        if hex.len() == SHA256_HEX_LEN && is_lower_hex(hex) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InvalidDigest)
        }
    }

    fn from_hasher(hasher: Sha256) -> Self {
        let bytes = hasher.finalize();
        let mut text = String::with_capacity(SHA256_PREFIX.len() + SHA256_HEX_LEN);
        text.push_str(SHA256_PREFIX);
        for byte in bytes.iter() {
            text.push(hex_digit(byte >> 4));
            text.push(hex_digit(byte & 0x0f));
        }
        Self(text)
    }

    /// The digest as written in `lock.toml`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether every character of `text` is a lowercase hexadecimal digit.
pub(crate) fn is_lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex_digit(nibble: u8) -> char {
    char::from(b"0123456789abcdef"[usize::from(nibble & 0x0f)])
}

impl TryFrom<String> for Digest {
    type Error = InvalidDigest;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

impl From<Digest> for String {
    fn from(digest: Digest) -> Self {
        digest.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A digest string that is not `sha256:<64 lowercase hex digits>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDigest;

impl fmt::Display for InvalidDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected `sha256:` followed by 64 lowercase hexadecimal digits")
    }
}

impl std::error::Error for InvalidDigest {}

/// Failure to hash a directory tree.
#[derive(Debug)]
#[non_exhaustive]
pub enum HashError {
    /// Reading a file or directory failed.
    Io {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// The tree contains a symbolic link. Materialised Skills hold regular
    /// files only.
    Symlink {
        /// Path of the link.
        path: PathBuf,
    },
    /// The tree contains something other than a regular file or directory.
    SpecialFile {
        /// Path of the entry.
        path: PathBuf,
    },
    /// The root of the tree is not a directory.
    NotADirectory {
        /// Path of the root.
        path: PathBuf,
    },
    /// A file or directory name is not valid UTF-8, so it has no portable
    /// representation.
    NonUtf8Name {
        /// Path of the entry.
        path: PathBuf,
    },
}

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Symlink { path } => write!(
                f,
                "{} is a symbolic link; Skill directories must hold regular files only",
                path.display()
            ),
            Self::SpecialFile { path } => write!(
                f,
                "{} is neither a regular file nor a directory",
                path.display()
            ),
            Self::NotADirectory { path } => write!(f, "{} is not a directory", path.display()),
            Self::NonUtf8Name { path } => {
                write!(f, "{} has a name that is not valid UTF-8", path.display())
            }
        }
    }
}

impl std::error::Error for HashError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> HashError + '_ {
    move |source| HashError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Feeds a length-prefixed byte string to the hasher.
fn update_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    update_len(hasher, bytes.len());
    hasher.update(bytes);
}

fn update_len(hasher: &mut Sha256, len: usize) {
    // `usize` is at most 64 bits on every supported platform.
    hasher.update((len as u64).to_be_bytes());
}

/// Hashes the directory tree rooted at `root`: every regular file's path
/// relative to `root` (with `/` separators) and contents, in sorted path
/// order.
///
/// # Errors
///
/// Returns a [`HashError`] when the tree cannot be read, `root` itself is a
/// symbolic link or not a directory, or the tree contains a symbolic link, a
/// special file, or a name that is not valid UTF-8.
pub fn tree_digest(root: &Path) -> Result<Digest, HashError> {
    let root_type = fs::symlink_metadata(root)
        .map_err(io_error(root))?
        .file_type();
    if root_type.is_symlink() {
        return Err(HashError::Symlink {
            path: root.to_path_buf(),
        });
    } else if !root_type.is_dir() {
        return Err(HashError::NotADirectory {
            path: root.to_path_buf(),
        });
    }
    let mut files = Vec::new();
    collect_files(root, "", &mut files)?;
    files.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    hasher.update(TREE_DOMAIN);
    update_len(&mut hasher, files.len());
    for (relative, path) in &files {
        let contents = fs::read(path).map_err(io_error(path))?;
        update_bytes(&mut hasher, relative.as_bytes());
        update_bytes(&mut hasher, &contents);
    }
    Ok(Digest::from_hasher(hasher))
}

/// Collects `(relative path, absolute path)` pairs for every regular file
/// below `dir`, refusing symbolic links and special files.
fn collect_files(
    dir: &Path,
    prefix: &str,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), HashError> {
    let entries = fs::read_dir(dir).map_err(io_error(dir))?;
    for entry in entries {
        let entry = entry.map_err(io_error(dir))?;
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| HashError::NonUtf8Name { path: path.clone() })?;
        let relative = format!("{prefix}{name}");
        let file_type = entry.file_type().map_err(io_error(&path))?;
        if file_type.is_symlink() {
            return Err(HashError::Symlink { path });
        } else if file_type.is_dir() {
            collect_files(&path, &format!("{relative}/"), files)?;
        } else if file_type.is_file() {
            files.push((relative, path));
        } else {
            return Err(HashError::SpecialFile { path });
        }
    }
    Ok(())
}

/// Hashes normalised Adaptation or Composition inputs: a structured
/// declaration and the written intent, if any.
///
/// The declaration is encoded canonically (tables by sorted key, every value
/// tagged with its type and length-prefixed), so the digest depends on what
/// the rules say, not on how the TOML that declared them was formatted or
/// ordered.
#[must_use]
pub fn inputs_digest(declaration: &toml::Value, intent: Option<&[u8]>) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(INPUTS_DOMAIN);
    update_value(&mut hasher, declaration);
    match intent {
        None => hasher.update([0]),
        Some(bytes) => {
            hasher.update([1]);
            update_bytes(&mut hasher, bytes);
        }
    }
    Digest::from_hasher(hasher)
}

fn update_value(hasher: &mut Sha256, value: &toml::Value) {
    match value {
        toml::Value::String(text) => {
            hasher.update(b"s");
            update_bytes(hasher, text.as_bytes());
        }
        toml::Value::Integer(number) => {
            hasher.update(b"i");
            hasher.update(number.to_be_bytes());
        }
        toml::Value::Float(number) => {
            hasher.update(b"f");
            hasher.update(number.to_bits().to_be_bytes());
        }
        toml::Value::Boolean(flag) => {
            hasher.update(b"b");
            hasher.update([u8::from(*flag)]);
        }
        toml::Value::Datetime(datetime) => {
            hasher.update(b"d");
            update_bytes(hasher, datetime.to_string().as_bytes());
        }
        toml::Value::Array(items) => {
            hasher.update(b"a");
            update_len(hasher, items.len());
            for item in items {
                update_value(hasher, item);
            }
        }
        toml::Value::Table(table) => {
            // Sorted explicitly: `toml::Table` keeps insertion order when the
            // `preserve_order` feature is enabled anywhere in the build.
            let mut entries = table.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            hasher.update(b"t");
            update_len(hasher, entries.len());
            for (key, item) in entries {
                update_bytes(hasher, key.as_bytes());
                update_value(hasher, item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    use super::*;

    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn tree(files: &[(&str, &[u8])]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (relative, contents) in files {
            write(dir.path(), relative, contents);
        }
        dir
    }

    const SKILL: &[(&str, &[u8])] = &[
        ("SKILL.md", b"---\nname: grill\n---\nBody\n"),
        ("scripts/run.sh", b"echo hi\n"),
        ("references/a.md", b"A"),
        ("references/b.md", b"B"),
    ];

    #[test]
    fn digest_strings_round_trip_and_reject_malformed_text() {
        let digest = tree_digest(tree(SKILL).path()).unwrap();
        assert_eq!(Digest::parse(digest.as_str()), Ok(digest.clone()));
        assert!(digest.as_str().starts_with("sha256:"));
        assert_eq!(digest.as_str().len(), 71);
        let good_hex = "0".repeat(64);
        for bad in [
            String::new(),
            good_hex.clone(),
            format!("sha1:{good_hex}"),
            format!("sha256:{}", "0".repeat(63)),
            format!("sha256:{}", "0".repeat(65)),
            format!("sha256:{}", "A".repeat(64)),
            format!("sha256:{}", "g".repeat(64)),
        ] {
            assert_eq!(Digest::parse(&bad), Err(InvalidDigest), "{bad:?}");
        }
    }

    #[test]
    fn tree_digest_ignores_file_creation_order() {
        let forward = tree(SKILL);
        let mut reversed_files = SKILL.to_vec();
        reversed_files.reverse();
        let reversed = tree(&reversed_files);
        assert_eq!(
            tree_digest(forward.path()).unwrap(),
            tree_digest(reversed.path()).unwrap()
        );
    }

    #[test]
    fn tree_digest_ignores_empty_directories() {
        let plain = tree(SKILL);
        let with_empty_dir = tree(SKILL);
        fs::create_dir_all(with_empty_dir.path().join("empty/nested")).unwrap();
        assert_eq!(
            tree_digest(plain.path()).unwrap(),
            tree_digest(with_empty_dir.path()).unwrap()
        );
    }

    #[test]
    fn tree_digest_detects_a_single_byte_change_in_any_file() {
        let original = tree_digest(tree(SKILL).path()).unwrap();
        for (index, (_, contents)) in SKILL.iter().enumerate() {
            for position in 0..contents.len() {
                let mut changed = SKILL.to_vec();
                let mut bytes = contents.to_vec();
                bytes[position] ^= 0x01;
                changed[index].1 = &bytes;
                let digest = tree_digest(tree(&changed).path()).unwrap();
                assert_ne!(digest, original, "file {index}, byte {position}");
            }
        }
    }

    #[test]
    fn tree_digest_detects_added_removed_and_renamed_files() {
        let original = tree_digest(tree(SKILL).path()).unwrap();

        let mut added = SKILL.to_vec();
        added.push(("extra.md", b""));
        assert_ne!(tree_digest(tree(&added).path()).unwrap(), original);

        let removed = &SKILL[..SKILL.len() - 1];
        assert_ne!(tree_digest(tree(removed).path()).unwrap(), original);

        let mut renamed = SKILL.to_vec();
        renamed[1].0 = "scripts/run2.sh";
        assert_ne!(tree_digest(tree(&renamed).path()).unwrap(), original);

        let mut moved = SKILL.to_vec();
        moved[2].0 = "references-a.md";
        assert_ne!(tree_digest(tree(&moved).path()).unwrap(), original);
    }

    #[test]
    fn tree_digest_does_not_confuse_path_and_content_boundaries() {
        let one = tree(&[("ab", b"c")]);
        let two = tree(&[("a", b"bc")]);
        assert_ne!(
            tree_digest(one.path()).unwrap(),
            tree_digest(two.path()).unwrap()
        );
    }

    #[test]
    fn tree_digest_rejects_symlinks() {
        let dir = tree(SKILL);
        symlink("/etc/passwd", dir.path().join("link")).unwrap();
        let err = tree_digest(dir.path()).unwrap_err();
        assert!(matches!(err, HashError::Symlink { ref path } if path.ends_with("link")));
        assert!(err.to_string().contains("symbolic link"));

        let dir = tree(SKILL);
        symlink(dir.path().join("references"), dir.path().join("refs")).unwrap();
        assert!(matches!(
            tree_digest(dir.path()),
            Err(HashError::Symlink { .. })
        ));
    }

    #[test]
    fn tree_digest_rejects_a_symlinked_or_non_directory_root() {
        let outside = tree(SKILL);
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("skill");
        symlink(outside.path(), &link).unwrap();
        let err = tree_digest(&link).unwrap_err();
        assert!(matches!(err, HashError::Symlink { ref path } if *path == link));

        let file = outside.path().join("SKILL.md");
        let err = tree_digest(&file).unwrap_err();
        assert!(matches!(err, HashError::NotADirectory { ref path } if *path == file));
        assert!(err.to_string().ends_with("is not a directory"));
    }

    #[test]
    fn tree_digest_reports_a_missing_root() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing");
        let err = tree_digest(&missing).unwrap_err();
        assert!(matches!(err, HashError::Io { ref path, .. } if *path == missing));
    }

    fn value(text: &str) -> toml::Value {
        toml::Value::Table(text.parse::<toml::Table>().unwrap())
    }

    #[test]
    fn inputs_digest_ignores_key_order_and_formatting() {
        let a =
            value("mode = 'adapt'\n[frontmatter]\nremove = ['icon']\nset = { a = 1, b = true }\n");
        let b = value("[frontmatter]\nset.b = true\nset.a = 1\nremove = [\"icon\"]\n\n[x]\n");
        let b = {
            let mut table = b.as_table().unwrap().clone();
            table.remove("x");
            table.insert("mode".into(), toml::Value::String("adapt".into()));
            toml::Value::Table(table)
        };
        assert_eq!(inputs_digest(&a, None), inputs_digest(&b, None));
    }

    #[test]
    fn inputs_digest_detects_rule_changes() {
        let base = value("[files]\nexclude = ['assets/**']\n[frontmatter]\nset = { n = 1 }\n");
        let original = inputs_digest(&base, None);
        for changed in [
            "[files]\nexclude = ['assets/*']\n[frontmatter]\nset = { n = 1 }\n",
            "[files]\nexclude = ['assets/**']\n[frontmatter]\nset = { n = 2 }\n",
            "[files]\nexclude = ['assets/**']\n[frontmatter]\nset = { n = '1' }\n",
            "[files]\nexclude = ['assets/**']\n[frontmatter]\nset = { n = 1.0 }\n",
            "[files]\nexclude = ['assets/**', 'x']\n[frontmatter]\nset = { n = 1 }\n",
            "[files]\nexclude = [['assets/**']]\n[frontmatter]\nset = { n = 1 }\n",
            "[files]\ninclude = ['assets/**']\n[frontmatter]\nset = { n = 1 }\n",
        ] {
            assert_ne!(inputs_digest(&value(changed), None), original, "{changed}");
        }
    }

    #[test]
    fn inputs_digest_detects_intent_changes() {
        let declaration = value("mode = 'adapt'\n");
        let intent = b"Keep turns short.\n".to_vec();
        let with_intent = inputs_digest(&declaration, Some(&intent));
        assert_ne!(with_intent, inputs_digest(&declaration, None));
        assert_ne!(
            inputs_digest(&declaration, Some(b"")),
            inputs_digest(&declaration, None)
        );
        for position in 0..intent.len() {
            let mut changed = intent.clone();
            changed[position] ^= 0x01;
            assert_ne!(inputs_digest(&declaration, Some(&changed)), with_intent);
        }
    }

    #[test]
    fn inputs_digest_is_stable() {
        // Pinned so an accidental change to the encoding, which would make
        // every recorded lock hash drift, fails loudly.
        let declaration =
            value("mode = 'copy'\nsource = 'mattpocock'\nupstream_path = 'skills/grill-me'\n");
        assert_eq!(
            inputs_digest(&declaration, None).as_str(),
            PINNED_INPUTS_DIGEST
        );
    }

    const PINNED_INPUTS_DIGEST: &str =
        "sha256:7a3294961e114a7bb512cd16b9a0cab5563f52f9c706217fe56fe9a4852efc24";

    #[test]
    fn tree_digest_is_stable() {
        // Pinned for the same reason as `inputs_digest_is_stable`; both values
        // were cross-checked against an independent implementation of the
        // documented encoding.
        assert_eq!(
            tree_digest(tree(SKILL).path()).unwrap().as_str(),
            PINNED_TREE_DIGEST
        );
    }

    const PINNED_TREE_DIGEST: &str =
        "sha256:f186d7b50a90663e39965487a45e4bebbe450b341d28570c5beeea7cb71c6e13";
}
