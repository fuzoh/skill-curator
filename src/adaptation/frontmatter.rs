//! D1: the YAML frontmatter of a `SKILL.md` and the deterministic rules that
//! set, remove, or keep only some of its top-level keys.
//!
//! A `SKILL.md` starts with a `---` line, the YAML frontmatter, and a closing
//! `---` line; everything after the closing line is the Markdown body. The
//! body is never parsed and always kept byte for byte, so fenced blocks,
//! horizontal rules (`---`), and any other Markdown stay intact.
//!
//! The frontmatter is edited entry by entry rather than re-serialised: each
//! top-level key owns the lines from its own line up to the next key, and
//! untouched entries keep their exact text, comments, and quoting. Only the
//! entries a rule sets are written by this module. Every edit is checked by
//! parsing the result again: the edited frontmatter must hold exactly the
//! expected entries, otherwise the edit is refused rather than risking a
//! frontmatter that means something else.
//!
//! Lines end at `\r\n`, `\r`, or `\n`, the YAML line breaks, so entry
//! lines always agree with the line numbers the YAML parser reports.

use std::collections::HashMap;
use std::fmt::{self, Write as _};

use saphyr_parser::{Event, Parser, ScalarStyle, ScanError};

use crate::collection::manifest::FrontmatterRules;

/// Delimiter line opening and closing the frontmatter.
const DELIMITER: &str = "---";
/// Byte order mark some editors put at the start of a file.
const BOM: char = '\u{feff}';
/// Deepest nesting of collections accepted in a frontmatter value. Skill
/// frontmatter is shallow; the limit keeps hostile upstream text from
/// exhausting the stack.
const MAX_DEPTH: usize = 64;
/// Most nodes a frontmatter may hold once its aliases are resolved. Aliases
/// are resolved by copying the anchored node, so the limit keeps hostile
/// upstream text (an alias of an alias of ...) from exhausting memory.
const MAX_NODES: usize = 100_000;

/// Why a `SKILL.md` frontmatter cannot be read or edited.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrontmatterError {
    /// The opening `---` line has no closing `---` line.
    Unclosed,
    /// The frontmatter is not valid YAML.
    Syntax {
        /// Line in `SKILL.md`, starting at 1.
        line: usize,
        /// What the YAML parser reported.
        message: String,
    },
    /// The frontmatter holds more than one YAML document.
    MultipleDocuments,
    /// The frontmatter is not a mapping of keys to values.
    NotAMapping,
    /// A top-level key is not a plain key at the start of its line, as in a
    /// block mapping (`key: value`).
    UnsupportedKey {
        /// Line in `SKILL.md`, starting at 1.
        line: usize,
    },
    /// A top-level key appears more than once.
    DuplicateKey {
        /// The repeated key.
        key: String,
    },
    /// A value nests collections deeper than this tool accepts.
    TooDeep {
        /// Line in `SKILL.md`, starting at 1.
        line: usize,
    },
    /// The frontmatter holds more nodes than this tool accepts once its
    /// aliases are resolved.
    TooLarge {
        /// Line in `SKILL.md`, starting at 1.
        line: usize,
    },
    /// Editing the frontmatter would change entries no rule touches, for
    /// example through anchors and aliases shared between entries.
    UnsafeEdit,
}

impl fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unclosed => write!(
                f,
                "the frontmatter opened by the first `{DELIMITER}` line has no closing `{DELIMITER}` line"
            ),
            Self::Syntax { line, message } => {
                write!(f, "the frontmatter is not valid YAML (line {line}): {message}")
            }
            Self::MultipleDocuments => {
                f.write_str("the frontmatter must hold a single YAML document")
            }
            Self::NotAMapping => f.write_str("the frontmatter must be a mapping of keys to values"),
            Self::UnsupportedKey { line } => write!(
                f,
                "the frontmatter key on line {line} must be plain text at the start of its line (`key: value`)"
            ),
            Self::DuplicateKey { key } => {
                write!(f, "the frontmatter key `{key}` appears more than once")
            }
            Self::TooDeep { line } => write!(
                f,
                "the frontmatter value on line {line} nests more than {MAX_DEPTH} levels deep"
            ),
            Self::TooLarge { line } => write!(
                f,
                "the frontmatter holds more than {MAX_NODES} values once its aliases are resolved (line {line})"
            ),
            Self::UnsafeEdit => f.write_str(
                "the frontmatter cannot be edited without changing entries the rules do not touch (anchors or aliases shared between keys?)",
            ),
        }
    }
}

impl std::error::Error for FrontmatterError {}

/// A parsed YAML node, compared by what it means to a YAML reader rather
/// than by how it is written: quoting matters only between plain scalars,
/// which YAML resolves to numbers, booleans, or null, and quoted or block
/// scalars, which are always strings. Aliases are resolved to a copy of the
/// node their anchor names, so an edit that makes an alias name another
/// anchor changes the aliasing entry's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// A scalar value.
    Scalar {
        /// The scalar's text, escapes resolved.
        value: String,
        /// Whether the scalar is plain (unquoted, not a block scalar).
        plain: bool,
        /// Explicit tag, such as `!!str`.
        tag: Option<String>,
    },
    /// A sequence of values.
    Sequence(Vec<Node>, Option<String>),
    /// A mapping, in written order.
    Mapping(Vec<(Node, Node)>, Option<String>),
}

/// One top-level frontmatter entry: its key and the lines it spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    key: String,
    /// The entry's lines, from its key's line up to the next key, each with
    /// its line ending.
    text: String,
    value: Node,
}

impl Entry {
    /// The key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The parsed value.
    #[must_use]
    pub fn value(&self) -> &Node {
        &self.value
    }
}

/// The frontmatter block between the delimiter lines.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frontmatter {
    /// The opening delimiter line, with its line ending.
    open: String,
    /// Lines before the first key (comments, blank lines).
    preamble: String,
    entries: Vec<Entry>,
    /// The closing delimiter line, with its line ending if any.
    close: String,
}

/// A `SKILL.md` split into its frontmatter entries and its verbatim body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    original: String,
    bom: bool,
    /// Line ending used for lines written into the frontmatter.
    newline: &'static str,
    frontmatter: Option<Frontmatter>,
    body: String,
    modified: bool,
}

impl SkillDocument {
    /// Splits `text` into frontmatter and body, and parses the frontmatter.
    ///
    /// A file that does not start with a `---` line has no frontmatter; its
    /// whole text is the body.
    ///
    /// # Errors
    ///
    /// Returns a [`FrontmatterError`] when the frontmatter is not closed, is
    /// not valid YAML, or is not a block mapping with unique plain keys.
    pub fn parse(text: &str) -> Result<Self, FrontmatterError> {
        let (bom, rest) = match text.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let Some((open, after_open)) = delimiter_line(rest) else {
            return Ok(Self {
                original: text.to_owned(),
                bom,
                newline: newline_of(rest),
                frontmatter: None,
                body: rest.to_owned(),
                modified: false,
            });
        };
        let newline = if open.ends_with("\r\n") { "\r\n" } else { "\n" };
        let mut yaml_len = 0;
        let mut close = None;
        for line in after_open.split_inclusive('\n') {
            if line.trim_end() == DELIMITER {
                close = Some(line);
                break;
            }
            yaml_len += line.len();
        }
        let close = close.ok_or(FrontmatterError::Unclosed)?;
        let yaml = &after_open[..yaml_len];
        let body = &after_open[yaml_len + close.len()..];
        let (preamble, entries) = split_entries(yaml)?;
        Ok(Self {
            original: text.to_owned(),
            bom,
            newline,
            frontmatter: Some(Frontmatter {
                open: open.to_owned(),
                preamble,
                entries,
                close: close.to_owned(),
            }),
            body: body.to_owned(),
            modified: false,
        })
    }

    /// Whether the file has a frontmatter block.
    #[must_use]
    pub fn has_frontmatter(&self) -> bool {
        self.frontmatter.is_some()
    }

    /// The top-level entries, in written order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        self.frontmatter
            .as_ref()
            .map_or(&[], |frontmatter| frontmatter.entries.as_slice())
    }

    /// The entry of `key`, if present.
    #[must_use]
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries().iter().find(|entry| entry.key == key)
    }

    /// The Markdown body, exactly as written.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Removes the entry of `key`, if present.
    pub fn remove(&mut self, key: &str) {
        self.retain(|entry_key| entry_key != key);
    }

    /// Keeps only the entries whose key `keep` accepts.
    fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        if let Some(frontmatter) = &mut self.frontmatter {
            let before = frontmatter.entries.len();
            frontmatter.entries.retain(|entry| keep(&entry.key));
            if frontmatter.entries.len() != before {
                self.modified = true;
            }
        }
    }

    /// Sets `key` to `value`: replaces the existing entry in place, or
    /// appends a new one, creating the frontmatter when the file has none.
    /// An existing entry that already holds the same value is left as
    /// written.
    ///
    /// # Errors
    ///
    /// Returns [`FrontmatterError::UnsafeEdit`] when the written entry does
    /// not read back as `value`.
    pub fn set(&mut self, key: &str, value: &toml::Value) -> Result<(), FrontmatterError> {
        let mut text = String::new();
        write_string(&mut text, key);
        text.push_str(": ");
        write_value(&mut text, value);
        text.push_str(self.newline);
        let (_, mut parsed) = split_entries(&text)?;
        let entry = match (parsed.pop(), parsed.is_empty()) {
            (Some(entry), true) if entry.key == key && matches_value(&entry.value, value) => entry,
            _ => return Err(FrontmatterError::UnsafeEdit),
        };
        self.put(entry);
        Ok(())
    }

    /// Puts `entry` (taken from another document) in place of the entry with
    /// the same key, or appends it. An existing entry with the same value is
    /// left as written.
    pub fn put(&mut self, mut entry: Entry) {
        if !entry.text.ends_with(['\n', '\r']) {
            entry.text.push_str(self.newline);
        }
        let newline = self.newline;
        let frontmatter = self.frontmatter.get_or_insert_with(|| Frontmatter {
            open: format!("{DELIMITER}{newline}"),
            preamble: String::new(),
            entries: Vec::new(),
            close: format!("{DELIMITER}{newline}"),
        });
        match frontmatter
            .entries
            .iter_mut()
            .find(|existing| existing.key == entry.key)
        {
            Some(existing) if existing.value == entry.value => return,
            Some(existing) => *existing = entry,
            None => frontmatter.entries.push(entry),
        }
        self.modified = true;
    }

    /// Applies one set of D1 rules: `remove`, then `keep_only`, then `set`.
    ///
    /// # Errors
    ///
    /// Returns [`FrontmatterError::UnsafeEdit`] when a set value does not
    /// read back as written.
    pub fn apply(&mut self, rules: &FrontmatterRules) -> Result<(), FrontmatterError> {
        for key in &rules.remove {
            self.remove(key);
        }
        if let Some(keep_only) = &rules.keep_only {
            self.retain(|key| keep_only.iter().any(|kept| kept == key));
        }
        for (key, value) in &rules.set {
            self.set(key, value)?;
        }
        Ok(())
    }

    /// The document as text. Unchanged documents are returned exactly as
    /// parsed; edited ones are checked by parsing them again.
    ///
    /// # Errors
    ///
    /// Returns [`FrontmatterError::UnsafeEdit`] when the edited text does not
    /// read back as the expected entries and body.
    pub fn render(&self) -> Result<String, FrontmatterError> {
        if !self.modified {
            return Ok(self.original.clone());
        }
        let mut text = String::new();
        if self.bom {
            text.push(BOM);
        }
        if let Some(frontmatter) = &self.frontmatter {
            text.push_str(&frontmatter.open);
            text.push_str(&frontmatter.preamble);
            for entry in &frontmatter.entries {
                text.push_str(&entry.text);
            }
            text.push_str(&frontmatter.close);
        }
        text.push_str(&self.body);
        let reread = Self::parse(&text).map_err(|_| FrontmatterError::UnsafeEdit)?;
        let same_entries = reread.entries().len() == self.entries().len()
            && reread
                .entries()
                .iter()
                .zip(self.entries())
                .all(|(a, b)| a.key == b.key && a.value == b.value);
        if same_entries && reread.body == self.body {
            Ok(text)
        } else {
            Err(FrontmatterError::UnsafeEdit)
        }
    }
}

/// Applies D1 rule sets in order to the `SKILL.md` text. Returns the text
/// unchanged, byte for byte, when no rule changes anything. Without rules
/// (every rule set empty) the text is not parsed at all, so a Skill without
/// D1 rules keeps a frontmatter this module cannot edit.
///
/// # Errors
///
/// Returns a [`FrontmatterError`] when the frontmatter cannot be read or
/// edited safely.
pub fn apply_rules(text: &str, rules: &[&FrontmatterRules]) -> Result<String, FrontmatterError> {
    if rules.iter().all(|rules| rules.is_empty()) {
        return Ok(text.to_owned());
    }
    let mut document = SkillDocument::parse(text)?;
    for rules in rules {
        document.apply(rules)?;
    }
    document.render()
}

/// Replays a change of D1 rules onto an accepted `SKILL.md` without
/// touching what the change does not concern.
///
/// When `upstream` (the upstream `SKILL.md` at the pinned Revision) is given,
/// every key whose value differs between the upstream frontmatter under the
/// `old` rules and under the `new` rules takes its value under the `new`
/// rules, or is removed when the new rules drop it. A key a removed rule
/// used to drop thus comes back with its upstream value. The `new` rules are
/// then enforced on the result. Without `upstream` (a Composition), only the
/// `new` rules are enforced.
///
/// # Errors
///
/// Returns a [`FrontmatterError`] when a frontmatter cannot be read or
/// edited safely.
pub fn replay_rules(
    accepted: &str,
    upstream: Option<&str>,
    old: &[&FrontmatterRules],
    new: &[&FrontmatterRules],
) -> Result<String, FrontmatterError> {
    let mut document = SkillDocument::parse(accepted)?;
    if let Some(upstream) = upstream {
        let under = |rules: &[&FrontmatterRules]| -> Result<SkillDocument, FrontmatterError> {
            let mut document = SkillDocument::parse(upstream)?;
            for rules in rules {
                document.apply(rules)?;
            }
            Ok(document)
        };
        let before = under(old)?;
        let after = under(new)?;
        let keys = before
            .entries()
            .iter()
            .chain(after.entries())
            .map(|entry| entry.key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            let new_entry = after.entry(&key);
            if before.entry(&key).map(Entry::value) == new_entry.map(Entry::value) {
                continue;
            }
            match new_entry {
                Some(entry) => document.put(entry.clone()),
                None => document.remove(&key),
            }
        }
    }
    for rules in new {
        document.apply(rules)?;
    }
    document.render()
}

/// Splits off a first line that is a delimiter line (`---`, optionally
/// followed by spaces), returning it with its line ending and the rest.
fn delimiter_line(text: &str) -> Option<(&str, &str)> {
    let end = text.find('\n')? + 1;
    let line = &text[..end];
    (line.trim_end() == DELIMITER).then(|| text.split_at(end))
}

/// Line ending of the first line of `text`, `\n` by default.
fn newline_of(text: &str) -> &'static str {
    match text.find('\n') {
        Some(end) if text[..end].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

/// Parses frontmatter YAML into the lines before the first key and the
/// top-level entries.
fn split_entries(yaml: &str) -> Result<(String, Vec<Entry>), FrontmatterError> {
    let keys = parse_top_level(yaml)?;
    let lines = yaml_lines(yaml);
    let mut starts = keys.iter().map(|(_, line, _)| *line).collect::<Vec<_>>();
    // Key lines must increase and fall within the text; anything else means
    // the parser and this split disagree, and the entries cannot be cut.
    let unsupported = |line: usize| FrontmatterError::UnsupportedKey {
        line: frontmatter_line(line),
    };
    if let Some(pair) = starts.windows(2).find(|pair| pair[0] >= pair[1]) {
        return Err(unsupported(pair[1]));
    }
    if let Some(&line) = starts.iter().find(|&&line| line == 0 || line > lines.len()) {
        return Err(unsupported(line));
    }
    let first = starts.first().copied().unwrap_or(lines.len() + 1);
    let preamble = lines
        .get(..first - 1)
        .ok_or_else(|| unsupported(first))?
        .concat();
    starts.push(lines.len() + 1);
    let entries = keys
        .into_iter()
        .zip(starts.windows(2))
        .map(|((key, line, value), range)| {
            let text = lines
                .get(range[0] - 1..range[1] - 1)
                .ok_or_else(|| unsupported(line))?
                .concat();
            Ok(Entry { key, text, value })
        })
        .collect::<Result<_, FrontmatterError>>()?;
    Ok((preamble, entries))
}

/// Splits `yaml` into lines, each with its line break, ending lines at
/// `\r\n`, `\r`, or `\n` as the YAML parser does.
fn yaml_lines(yaml: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = yaml;
    while let Some(at) = rest.find(['\r', '\n']) {
        let len = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        let (line, after) = rest.split_at(at + len);
        lines.push(line);
        rest = after;
    }
    if !rest.is_empty() {
        lines.push(rest);
    }
    lines
}

/// Line number in `SKILL.md` of line `line` of the frontmatter YAML, which
/// starts after the opening delimiter.
fn frontmatter_line(line: usize) -> usize {
    line + 1
}

/// Parses the top-level mapping of `yaml`: each key, the line it starts on
/// (1-based, within `yaml`), and its value. An empty document is an empty
/// mapping.
fn parse_top_level(yaml: &str) -> Result<Vec<(String, usize, Node)>, FrontmatterError> {
    let mut events = Events {
        parser: Parser::new_from_str(yaml),
        anchors: HashMap::new(),
        nodes: 0,
    };
    if events.next()?.0 != Event::StreamStart {
        return Err(FrontmatterError::NotAMapping);
    }
    let entries = match events.next()?.0 {
        Event::StreamEnd => return Ok(Vec::new()),
        Event::DocumentStart(_) => match events.next()?.0 {
            Event::MappingStart(_, _) => top_level_entries(&mut events)?,
            _ => return Err(FrontmatterError::NotAMapping),
        },
        _ => return Err(FrontmatterError::NotAMapping),
    };
    if events.next()?.0 != Event::DocumentEnd {
        return Err(FrontmatterError::NotAMapping);
    }
    match events.next()?.0 {
        Event::StreamEnd => Ok(entries),
        _ => Err(FrontmatterError::MultipleDocuments),
    }
}

fn top_level_entries(
    events: &mut Events<'_>,
) -> Result<Vec<(String, usize, Node)>, FrontmatterError> {
    let mut entries: Vec<(String, usize, Node)> = Vec::new();
    loop {
        let (event, line, col) = events.next()?;
        let key = match event {
            Event::MappingEnd => return Ok(entries),
            Event::Scalar(value, style, anchor, tag) if col == 0 => {
                let key = value.into_owned();
                let node = Node::Scalar {
                    value: key.clone(),
                    plain: style == ScalarStyle::Plain,
                    tag: tag.map(|tag| tag.to_string()),
                };
                events.add_node(&node, line, anchor, 1)?;
                key
            }
            _ => {
                return Err(FrontmatterError::UnsupportedKey {
                    line: frontmatter_line(line),
                });
            }
        };
        if entries.iter().any(|(existing, _, _)| *existing == key) {
            return Err(FrontmatterError::DuplicateKey { key });
        }
        let value = events.node(0)?;
        entries.push((key, line, value));
    }
}

/// The parser's events, with errors mapped to [`FrontmatterError`], and the
/// anchored nodes seen so far, by anchor id.
struct Events<'input> {
    parser: Parser<'input, saphyr_parser::StrInput<'input>>,
    /// Each anchored node and its size in nodes. The parser gives every
    /// anchor definition its own id, so a redefined anchor name gets a new
    /// entry and later aliases resolve to it.
    anchors: HashMap<usize, (Node, usize)>,
    /// Nodes built so far, aliases counted at their resolved size.
    nodes: usize,
}

impl<'input> Events<'input> {
    /// The next event, the line it starts on (1-based), and its column
    /// (0-based).
    fn next(&mut self) -> Result<(Event<'input>, usize, usize), FrontmatterError> {
        match self.parser.next_event() {
            Some(Ok((event, span))) => Ok((event, span.start.line(), span.start.col())),
            Some(Err(err)) => Err(syntax_error(&err)),
            None => Err(FrontmatterError::NotAMapping),
        }
    }

    /// Counts `size` more nodes, and records `node` under `anchor` when it
    /// has one (anchor id 0 means none).
    fn add_node(
        &mut self,
        node: &Node,
        line: usize,
        anchor: usize,
        size: usize,
    ) -> Result<(), FrontmatterError> {
        self.nodes = self.nodes.saturating_add(size);
        if self.nodes > MAX_NODES {
            return Err(FrontmatterError::TooLarge {
                line: frontmatter_line(line),
            });
        }
        if anchor != 0 {
            self.anchors.insert(anchor, (node.clone(), size));
        }
        Ok(())
    }

    /// Reads one complete node, starting with the next event, with its
    /// aliases resolved.
    fn node(&mut self, depth: usize) -> Result<Node, FrontmatterError> {
        let (event, line, _) = self.next()?;
        if depth > MAX_DEPTH {
            return Err(FrontmatterError::TooDeep {
                line: frontmatter_line(line),
            });
        }
        let before = self.nodes;
        let (node, anchor) = match event {
            Event::Scalar(value, style, anchor, tag) => (
                Node::Scalar {
                    value: value.into_owned(),
                    plain: style == ScalarStyle::Plain,
                    tag: tag.map(|tag| tag.to_string()),
                },
                anchor,
            ),
            Event::Alias(anchor) => {
                // An alias inside the node its anchor names (a recursive
                // alias) has no complete target yet.
                let Some((target, size)) = self.anchors.get(&anchor).cloned() else {
                    return Err(FrontmatterError::Syntax {
                        line: frontmatter_line(line),
                        message: "an alias refers to a node that contains it".to_owned(),
                    });
                };
                self.add_node(&target, line, 0, size)?;
                return Ok(target);
            }
            Event::SequenceStart(anchor, tag) => {
                let mut items = Vec::new();
                loop {
                    if matches!(self.parser.peek(), Some(Ok((Event::SequenceEnd, _)))) {
                        self.next()?;
                        break;
                    }
                    items.push(self.node(depth + 1)?);
                }
                (
                    Node::Sequence(items, tag.map(|tag| tag.to_string())),
                    anchor,
                )
            }
            Event::MappingStart(anchor, tag) => {
                let mut pairs = Vec::new();
                loop {
                    if matches!(self.parser.peek(), Some(Ok((Event::MappingEnd, _)))) {
                        self.next()?;
                        break;
                    }
                    let key = self.node(depth + 1)?;
                    let value = self.node(depth + 1)?;
                    pairs.push((key, value));
                }
                (Node::Mapping(pairs, tag.map(|tag| tag.to_string())), anchor)
            }
            _ => {
                return Err(FrontmatterError::Syntax {
                    line: frontmatter_line(line),
                    message: "unexpected YAML structure".to_owned(),
                });
            }
        };
        // The node itself, plus the nodes its children added.
        let size = self.nodes - before + 1;
        self.nodes = before;
        self.add_node(&node, line, anchor, size)?;
        Ok(node)
    }
}

fn syntax_error(err: &ScanError) -> FrontmatterError {
    FrontmatterError::Syntax {
        line: frontmatter_line(err.marker().line()),
        message: err.info().to_owned(),
    }
}

/// Writes `value` as a single-line YAML flow value.
fn write_value(out: &mut String, value: &toml::Value) {
    match value {
        toml::Value::String(text) => write_string(out, text),
        toml::Value::Integer(number) => {
            let _ = write!(out, "{number}");
        }
        toml::Value::Float(number) => out.push_str(&float_text(*number)),
        toml::Value::Boolean(flag) => out.push_str(if *flag { "true" } else { "false" }),
        toml::Value::Datetime(datetime) => write_quoted(out, &datetime.to_string()),
        toml::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_value(out, item);
            }
            out.push(']');
        }
        toml::Value::Table(table) => {
            out.push('{');
            for (index, (key, item)) in table.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

/// YAML text of a float: the shortest form that reads back as a float.
fn float_text(number: f64) -> String {
    if number.is_nan() {
        ".nan".to_owned()
    } else if number.is_infinite() {
        if number > 0.0 { ".inf" } else { "-.inf" }.to_owned()
    } else {
        // `Debug` always writes a fraction or an exponent (`1.0`, `1e100`),
        // so the text never reads back as an integer.
        format!("{number:?}")
    }
}

/// Writes a string plain when that is unambiguous, double-quoted otherwise.
fn write_string(out: &mut String, text: &str) {
    if is_plain_safe(text) {
        out.push_str(text);
    } else {
        write_quoted(out, text);
    }
}

/// Whether `text` reads back as the same string when written unquoted, in
/// block and flow context, under YAML 1.2 and YAML 1.1 readers alike: it
/// starts with an ASCII letter, holds only letters, digits, spaces, `_`,
/// `.`, `-`, and `/`, does not end with a space, and is not a word YAML 1.1
/// reads as a boolean or null.
fn is_plain_safe(text: &str) -> bool {
    const RESERVED: &[&str] = &["y", "n", "yes", "no", "true", "false", "on", "off", "null"];
    let starts_with_letter = text.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    let allowed = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-' | '/'));
    starts_with_letter
        && allowed
        && !text.ends_with(' ')
        && !RESERVED.contains(&text.to_ascii_lowercase().as_str())
}

/// Writes a YAML double-quoted string, escaping quotes, backslashes, control
/// characters, and the characters YAML treats as line breaks.
fn write_quoted(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            c if c.is_control() || c == BOM => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Whether a parsed node is what [`write_value`] meant to write for
/// `value`.
fn matches_value(node: &Node, value: &toml::Value) -> bool {
    let scalar = |text: &str, want_plain: bool| matches!(node, Node::Scalar { value, plain, tag: None } if value == text && *plain == want_plain);
    match value {
        toml::Value::String(text) => {
            matches!(node, Node::Scalar { value, plain, tag: None }
                if value == text && (!*plain || is_plain_safe(text)))
        }
        toml::Value::Integer(number) => scalar(&number.to_string(), true),
        toml::Value::Float(number) => scalar(&float_text(*number), true),
        toml::Value::Boolean(flag) => scalar(if *flag { "true" } else { "false" }, true),
        toml::Value::Datetime(datetime) => scalar(&datetime.to_string(), false),
        toml::Value::Array(items) => matches!(node, Node::Sequence(nodes, None)
            if nodes.len() == items.len()
                && nodes.iter().zip(items).all(|(node, item)| matches_value(node, item))),
        toml::Value::Table(table) => matches!(node, Node::Mapping(pairs, None)
        if pairs.len() == table.len()
            && pairs.iter().zip(table).all(|((key, node), (name, item))| {
                matches!(key, Node::Scalar { value, tag: None, .. } if value == name)
                    && matches_value(node, item)
            })),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A body exercising Markdown that a frontmatter parser could mangle:
    /// horizontal rules, a `---` line, `!` fences, and trailing spaces.
    const BODY: &str = "# Grill me\n\n---\n\nAsk one question at a time.  \n\n```!\nrun --this\n```\n\n***\n---\nEnd without newline";

    fn skill(frontmatter: &str) -> String {
        format!("---\n{frontmatter}---\n{BODY}")
    }

    fn rules(
        set: &[(&str, toml::Value)],
        remove: &[&str],
        keep_only: Option<&[&str]>,
    ) -> FrontmatterRules {
        FrontmatterRules {
            set: set
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
            remove: remove.iter().map(|key| (*key).to_owned()).collect(),
            keep_only: keep_only.map(|keys| keys.iter().map(|key| (*key).to_owned()).collect()),
        }
    }

    fn text(value: &str) -> toml::Value {
        toml::Value::String(value.to_owned())
    }

    const FRONTMATTER: &str = "# upstream comment\nname: grill-me\ndescription: >\n  Grill the user\n  relentlessly.\n# about tools\nallowed-tools: [Read, Grep]\nmetadata:\n  author: someone\n  version: '1.0'\n";

    #[test]
    fn untouched_documents_are_returned_byte_for_byte() {
        let original = skill(FRONTMATTER);
        assert_eq!(apply_rules(&original, &[]).unwrap(), original);
        let no_op = rules(&[], &["absent"], None);
        assert_eq!(apply_rules(&original, &[&no_op]).unwrap(), original);
        let crlf = "---\r\nname: x\r\n---\r\nbody\r\n";
        assert_eq!(apply_rules(crlf, &[&no_op]).unwrap(), crlf);
    }

    #[test]
    fn remove_drops_the_whole_entry_and_keeps_the_body_verbatim() {
        let original = skill(FRONTMATTER);
        let edited = apply_rules(
            &original,
            &[&rules(&[], &["description", "metadata"], None)],
        )
        .unwrap();
        assert_eq!(
            edited,
            skill("# upstream comment\nname: grill-me\nallowed-tools: [Read, Grep]\n")
        );
        assert!(edited.ends_with(BODY));
    }

    #[test]
    fn keep_only_keeps_listed_keys_in_their_written_order() {
        let original = skill(FRONTMATTER);
        let edited =
            apply_rules(&original, &[&rules(&[], &[], Some(&["metadata", "name"]))]).unwrap();
        assert_eq!(
            edited,
            skill(
                "# upstream comment\nname: grill-me\nmetadata:\n  author: someone\n  version: '1.0'\n"
            )
        );
    }

    #[test]
    fn set_replaces_in_place_and_appends_new_keys() {
        let original = skill(FRONTMATTER);
        let edited = apply_rules(
            &original,
            &[&rules(
                &[
                    ("name", text("grill")),
                    ("disable-model-invocation", toml::Value::Boolean(true)),
                ],
                &[],
                None,
            )],
        )
        .unwrap();
        assert_eq!(
            edited,
            skill(
                "# upstream comment\nname: grill\ndescription: >\n  Grill the user\n  relentlessly.\n# about tools\nallowed-tools: [Read, Grep]\nmetadata:\n  author: someone\n  version: '1.0'\ndisable-model-invocation: true\n"
            )
        );
    }

    #[test]
    fn set_keeps_an_entry_that_already_has_the_value() {
        let original = skill("name: grill-me # keep this comment\n");
        let same = rules(&[("name", text("grill-me"))], &[], None);
        assert_eq!(apply_rules(&original, &[&same]).unwrap(), original);
    }

    #[test]
    fn rule_sets_apply_in_order_remove_then_keep_only_then_set() {
        let original = skill("name: a\ndescription: b\nlicense: MIT\n");
        let first = rules(&[("license", text("Apache-2.0"))], &[], None);
        let second = rules(
            &[("description", text("Short."))],
            &["name"],
            Some(&["description", "license"]),
        );
        assert_eq!(
            apply_rules(&original, &[&first, &second]).unwrap(),
            skill("description: Short.\nlicense: Apache-2.0\n")
        );
    }

    #[test]
    fn set_creates_a_frontmatter_when_there_is_none() {
        let edited = apply_rules(BODY, &[&rules(&[("name", text("x"))], &[], None)]).unwrap();
        assert_eq!(edited, format!("---\nname: x\n---\n{BODY}"));
        let untouched = apply_rules(BODY, &[&rules(&[], &["name"], Some(&[]))]).unwrap();
        assert_eq!(untouched, BODY);
    }

    #[test]
    fn crlf_frontmatter_gets_crlf_entries() {
        let original = "---\r\nname: x\r\n---\r\nbody\r\n";
        let edited =
            apply_rules(original, &[&rules(&[("license", text("MIT"))], &[], None)]).unwrap();
        assert_eq!(edited, "---\r\nname: x\r\nlicense: MIT\r\n---\r\nbody\r\n");
    }

    #[test]
    fn values_are_written_so_they_read_back_exactly() {
        let mut table = toml::map::Map::new();
        table.insert("a b".to_owned(), toml::Value::Integer(-3));
        table.insert("plain".to_owned(), text("x"));
        let values = [
            text("plain words-and_dots.v2/x"),
            text("true"),
            text("Yes"),
            text("null"),
            text("123"),
            text("needs: quoting # here"),
            text(""),
            text("trailing "),
            text("line\nbreak\t\"quoted\" back\\slash \u{7f} \u{2028} é"),
            text("[not, a, list]"),
            toml::Value::Integer(42),
            toml::Value::Float(1.0),
            toml::Value::Float(1e100),
            toml::Value::Float(-0.5),
            toml::Value::Float(f64::INFINITY),
            toml::Value::Float(f64::NEG_INFINITY),
            toml::Value::Boolean(false),
            toml::Value::Datetime("1979-05-27T07:32:00Z".parse().unwrap()),
            toml::Value::Array(vec![text("Read"), text("a, b"), toml::Value::Array(vec![])]),
            toml::Value::Table(table),
            toml::Value::Table(toml::map::Map::new()),
        ];
        for value in values {
            let mut document = SkillDocument::parse(&skill("name: x\n")).unwrap();
            document.set("key", &value).unwrap();
            let reread = SkillDocument::parse(&document.render().unwrap()).unwrap();
            assert!(
                matches_value(reread.entry("key").unwrap().value(), &value),
                "{value:?} did not read back"
            );
            assert_eq!(reread.body(), BODY);
        }
    }

    #[test]
    fn plain_strings_are_written_unquoted_and_others_quoted() {
        let mut out = String::new();
        write_string(&mut out, "Grill the user");
        assert_eq!(out, "Grill the user");
        for (raw, quoted) in [
            ("on", "\"on\""),
            ("1.0", "\"1.0\""),
            ("a: b", "\"a: b\""),
            ("tab\there", "\"tab\\there\""),
            ("x\u{1}", "\"x\\u0001\""),
        ] {
            let mut out = String::new();
            write_string(&mut out, raw);
            assert_eq!(out, quoted, "{raw:?}");
        }
    }

    #[test]
    fn floats_never_read_back_as_integers() {
        assert_eq!(float_text(1.0), "1.0");
        assert_eq!(float_text(f64::NAN), ".nan");
        assert_eq!(float_text(f64::INFINITY), ".inf");
        assert_eq!(float_text(f64::NEG_INFINITY), "-.inf");
    }

    #[test]
    fn a_horizontal_rule_in_the_body_is_not_a_frontmatter() {
        let text = "# Title\n\n---\n\nname: not frontmatter\n---\n";
        let document = SkillDocument::parse(text).unwrap();
        assert!(!document.has_frontmatter());
        assert_eq!(document.body(), text);
    }

    #[test]
    fn delimiter_lines_inside_block_scalars_do_not_close_the_frontmatter() {
        let original = skill("description: |\n  ---\n  indented\nname: x\n");
        let edited = apply_rules(&original, &[&rules(&[], &["name"], None)]).unwrap();
        assert_eq!(edited, skill("description: |\n  ---\n  indented\n"));
    }

    #[test]
    fn a_byte_order_mark_is_kept() {
        let original = format!("\u{feff}{}", skill("name: x\n"));
        let edited = apply_rules(&original, &[&rules(&[], &["name"], None)]).unwrap();
        assert_eq!(edited, format!("\u{feff}{}", skill("")));
    }

    #[test]
    fn empty_frontmatter_is_an_empty_mapping() {
        let document = SkillDocument::parse(&skill("# only a comment\n")).unwrap();
        assert!(document.has_frontmatter());
        assert!(document.entries().is_empty());
        let edited = apply_rules(&skill(""), &[&rules(&[("name", text("x"))], &[], None)]).unwrap();
        assert_eq!(edited, skill("name: x\n"));
    }

    #[test]
    fn malformed_frontmatter_is_rejected_with_a_reason() {
        let cases = [
            ("---\nname: x\n".to_owned(), FrontmatterError::Unclosed),
            (skill("- a\n- b\n"), FrontmatterError::NotAMapping),
            (skill("plain text\n"), FrontmatterError::NotAMapping),
            (
                skill("name: a\nname: b\n"),
                FrontmatterError::DuplicateKey {
                    key: "name".to_owned(),
                },
            ),
            (
                skill("{name: a}\n"),
                FrontmatterError::UnsupportedKey { line: 2 },
            ),
            (
                skill("? name\n: a\n"),
                FrontmatterError::UnsupportedKey { line: 2 },
            ),
            (
                skill("a: 1\n...\nb: 2\n"),
                FrontmatterError::MultipleDocuments,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(SkillDocument::parse(&text), Err(expected), "{text:?}");
        }
        assert!(matches!(
            SkillDocument::parse(&skill("name: [unclosed\n")),
            Err(FrontmatterError::Syntax { .. })
        ));
    }

    #[test]
    fn deeply_nested_values_are_refused() {
        let deep = format!("a: {}{}\n", "[".repeat(200), "]".repeat(200));
        assert!(matches!(
            SkillDocument::parse(&skill(&deep)),
            Err(FrontmatterError::TooDeep { .. } | FrontmatterError::Syntax { .. })
        ));
    }

    #[test]
    fn edits_that_would_break_aliases_are_refused() {
        let original = skill("base: &b x\nname: *b\n");
        assert_eq!(
            apply_rules(&original, &[&rules(&[], &["base"], None)]),
            Err(FrontmatterError::UnsafeEdit)
        );
    }

    #[test]
    fn an_edit_that_makes_an_alias_name_another_anchor_is_refused() {
        // `b` names the second `&x`; removing it would make `b` name the
        // first one, changing `b` from `two` to `one`.
        let original = skill("a: &x one\nc: &x two\nb: *x\n");
        assert_eq!(
            apply_rules(&original, &[&rules(&[], &["c"], None)]),
            Err(FrontmatterError::UnsafeEdit)
        );
        // Aliases mean the node they name: an untouched alias whose anchor
        // stays is kept.
        let document = SkillDocument::parse(&skill("a: &x [1, 2]\nb: *x\nc: 3\n")).unwrap();
        assert_eq!(
            document.entry("b").unwrap().value(),
            document.entry("a").unwrap().value()
        );
        assert_eq!(
            apply_rules(
                &skill("a: &x [1, 2]\nb: *x\nc: 3\n"),
                &[&rules(&[], &["c"], None)]
            )
            .unwrap(),
            skill("a: &x [1, 2]\nb: *x\n")
        );
    }

    #[test]
    fn aliases_that_expand_too_far_are_refused() {
        // Each level doubles the previous one: resolving every alias would
        // need 2^40 nodes.
        let mut frontmatter = "l0: &l0 [x, x]\n".to_owned();
        for level in 1..=40 {
            let previous = level - 1;
            frontmatter.push_str(&format!(
                "l{level}: &l{level} [*l{previous}, *l{previous}]\n"
            ));
        }
        assert!(matches!(
            SkillDocument::parse(&skill(&frontmatter)),
            Err(FrontmatterError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_lone_carriage_return_ends_a_line_as_in_yaml() {
        let original = "---\na: 1\rb: 2\rc: 3\n---\nbody\n";
        let document = SkillDocument::parse(original).unwrap();
        let keys = document
            .entries()
            .iter()
            .map(Entry::key)
            .collect::<Vec<_>>();
        assert_eq!(keys, ["a", "b", "c"]);
        assert_eq!(
            apply_rules(original, &[&rules(&[], &["b"], None)]).unwrap(),
            "---\na: 1\rc: 3\n---\nbody\n"
        );
        let original = "---\nx: 0\na: 1\rb: 2\rc: 3\nd: 4\n---\nbody\n";
        assert_eq!(
            apply_rules(original, &[&rules(&[], &["a", "d"], None)]).unwrap(),
            "---\nx: 0\nb: 2\rc: 3\n---\nbody\n"
        );
        let crlf_mix = "---\r\na: 1\r\rb: 2\r\n---\r\nbody\r\n";
        assert_eq!(
            apply_rules(crlf_mix, &[&rules(&[], &["a"], None)]).unwrap(),
            "---\r\nb: 2\r\n---\r\nbody\r\n"
        );
    }

    #[test]
    fn yaml_lines_end_at_every_yaml_line_break() {
        assert_eq!(
            yaml_lines("a\nb\r\nc\rd\r\r\ne"),
            ["a\n", "b\r\n", "c\r", "d\r", "\r\n", "e"]
        );
        assert!(yaml_lines("").is_empty());
    }

    #[test]
    fn without_rules_the_frontmatter_is_not_parsed() {
        // Not something this module can edit: a plain scalar holding `: `.
        let lax = "---\nname: x\ndescription: Use when: the user asks\n---\nbody\n";
        assert!(SkillDocument::parse(lax).is_err());
        assert_eq!(apply_rules(lax, &[]).unwrap(), lax);
        let empty = FrontmatterRules::default();
        assert_eq!(apply_rules(lax, &[&empty, &empty]).unwrap(), lax);
        assert!(apply_rules(lax, &[&rules(&[], &["absent"], None)]).is_err());
    }

    #[test]
    fn replay_restores_keys_a_removed_rule_used_to_drop() {
        let upstream = skill("name: grill-me\ndescription: Upstream.\nallowed-tools: Read\n");
        // The accepted Skill was rewritten by an agent under the old rules.
        let accepted = skill("name: grill-me\ndescription: Rewritten.\n");
        let old = rules(&[], &["allowed-tools"], None);
        let new = rules(&[("license", text("MIT"))], &[], None);
        assert_eq!(
            replay_rules(&accepted, Some(&upstream), &[&old], &[&new]).unwrap(),
            skill("name: grill-me\ndescription: Rewritten.\nallowed-tools: Read\nlicense: MIT\n")
        );
    }

    #[test]
    fn replay_keeps_rewritten_keys_the_rule_change_does_not_concern() {
        let upstream = skill("name: grill-me\ndescription: Upstream.\nlicense: MIT\n");
        let accepted = skill("name: grill-me\ndescription: Rewritten.\nlicense: MIT\n");
        let old = rules(&[("license", text("MIT"))], &[], None);
        let new = rules(&[], &["license"], None);
        assert_eq!(
            replay_rules(&accepted, Some(&upstream), &[&old], &[&new]).unwrap(),
            skill("name: grill-me\ndescription: Rewritten.\n")
        );
        // A key the old rules set and the new rules leave alone returns to
        // its upstream value.
        let upstream = skill("name: grill-me\nlicense: Apache-2.0\n");
        let accepted = skill("name: grill-me\nlicense: MIT\n");
        assert_eq!(
            replay_rules(&accepted, Some(&upstream), &[&old], &[]).unwrap(),
            skill("name: grill-me\nlicense: Apache-2.0\n")
        );
    }

    #[test]
    fn replay_without_upstream_enforces_the_new_rules_only() {
        let accepted = skill("name: composed\nextra: x\n");
        let new = rules(&[], &[], Some(&["name"]));
        assert_eq!(
            replay_rules(&accepted, None, &[], &[&new]).unwrap(),
            skill("name: composed\n")
        );
    }
}
