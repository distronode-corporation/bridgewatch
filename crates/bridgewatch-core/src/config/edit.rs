//! Typed, format-preserving edits to `config.toml`.
//!
//! The GUI exposes the same keys as the file, so it has to write the file back.
//! Every change goes through `toml_edit`, which means hand-written comments, key
//! order and `[[watches]]` order survive: editing `poll.live_secs` in the
//! settings pane must not silently reflow somebody's annotated config.
//!
//! ```no_run
//! use bridgewatch_core::config::edit::{ConfigEditor, Edit, EditValue};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let raw = std::fs::read_to_string("config.toml")?;
//! let mut editor = ConfigEditor::new(&raw)?;
//! editor.apply(&[Edit::Set {
//!     path: "watches.1.poll.live_secs".into(),
//!     value: EditValue::Integer(30),
//! }])?;
//! std::fs::write("config.toml", editor.to_toml())?;
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

use super::schema::Watch;

/// Something that went wrong applying an edit.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// The document is not valid TOML.
    #[error("not valid TOML: {0}")]
    Parse(String),
    /// A path segment did not exist and could not be created.
    #[error("no such key: {path}")]
    NoSuchKey {
        /// The dotted path that could not be resolved.
        path: String,
    },
    /// A path tried to descend through something that is not a table.
    #[error("{path} is not a table, so {segment:?} cannot be set inside it")]
    NotATable {
        /// The dotted path resolved so far.
        path: String,
        /// The segment that could not be entered.
        segment: String,
    },
    /// `watches` is missing or is not an array of tables.
    #[error("the document has no [[watches]] array")]
    NoWatches,
    /// A watch id was not found.
    #[error("no watch with id {0:?}")]
    NoSuchWatch(String),
    /// A value could not be turned into TOML.
    #[error("cannot serialise value: {0}")]
    Serialize(String),
}

/// A scalar or list to write into the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditValue {
    /// A TOML string.
    String(String),
    /// A TOML integer.
    Integer(i64),
    /// A TOML float.
    Float(f64),
    /// A TOML boolean.
    Boolean(bool),
    /// A TOML array. Nested arrays are allowed.
    Array(Vec<EditValue>),
}

impl EditValue {
    fn to_value(&self) -> Value {
        match self {
            EditValue::String(s) => Value::from(s.as_str()),
            EditValue::Integer(i) => Value::from(*i),
            EditValue::Float(f) => Value::from(*f),
            EditValue::Boolean(b) => Value::from(*b),
            EditValue::Array(items) => {
                let mut arr = toml_edit::Array::new();
                for i in items {
                    arr.push(i.to_value());
                }
                Value::Array(arr)
            }
        }
    }
}

/// One change to apply. Serialisable so the GUI can send a batch over IPC and
/// the core can apply it atomically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum Edit {
    /// Set a dotted path. Missing intermediate tables are created inline.
    ///
    /// A numeric segment indexes an array of tables, so
    /// `watches.1.poll.live_secs` is the second watch's live interval.
    Set {
        /// Dotted path to the key.
        path: String,
        /// The value to write.
        value: EditValue,
    },
    /// Remove a dotted path. Removing something absent is not an error.
    Unset {
        /// Dotted path to the key.
        path: String,
    },
    /// Append a watch to `[[watches]]`.
    AddWatch {
        /// The watch to append.
        watch: Box<Watch>,
    },
    /// Remove the watch with this `id`.
    RemoveWatch {
        /// The watch id.
        id: String,
    },
    /// Set a key on the watch with this `id`, without having to know its index.
    SetWatch {
        /// The watch id.
        id: String,
        /// Dotted path relative to the watch table, e.g. `poll.live_secs`.
        path: String,
        /// The value to write.
        value: EditValue,
    },
}

/// A configuration document open for editing.
#[derive(Debug, Clone)]
pub struct ConfigEditor {
    doc: DocumentMut,
}

impl ConfigEditor {
    /// Open a TOML document for editing.
    pub fn new(raw: &str) -> Result<Self, EditError> {
        let doc = raw
            .parse::<DocumentMut>()
            .map_err(|e| EditError::Parse(e.to_string()))?;
        Ok(Self { doc })
    }

    /// The document as TOML, with every byte bridgewatch did not change left
    /// exactly as it was found.
    pub fn to_toml(&self) -> String {
        self.doc.to_string()
    }

    /// The underlying document, for a caller that needs something this API does
    /// not cover.
    pub fn document(&self) -> &DocumentMut {
        &self.doc
    }

    /// Apply a batch of edits in order. The first failure aborts, leaving the
    /// editor holding the partially applied document; callers that need
    /// all-or-nothing should clone first, which is cheap relative to a write.
    pub fn apply(&mut self, edits: &[Edit]) -> Result<(), EditError> {
        for edit in edits {
            match edit {
                Edit::Set { path, value } => self.set(path, value)?,
                Edit::Unset { path } => {
                    self.unset(path)?;
                }
                Edit::AddWatch { watch } => self.add_watch(watch)?,
                Edit::RemoveWatch { id } => {
                    self.remove_watch(id)?;
                }
                Edit::SetWatch { id, path, value } => {
                    let index = self
                        .watch_index(id)
                        .ok_or_else(|| EditError::NoSuchWatch(id.clone()))?;
                    self.set(&format!("watches.{index}.{path}"), value)?;
                }
            }
        }
        Ok(())
    }

    /// Set a dotted path.
    pub fn set(&mut self, path: &str, value: &EditValue) -> Result<(), EditError> {
        let segments = split_path(path);
        if segments.is_empty() {
            return Err(EditError::NoSuchKey {
                path: path.to_string(),
            });
        }
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        set_in_table(self.doc.as_table_mut(), &refs, value.to_value(), path, true)
    }

    /// Remove a dotted path. Returns whether anything was there.
    pub fn unset(&mut self, path: &str) -> Result<bool, EditError> {
        let segments = split_path(path);
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        unset_in_table(self.doc.as_table_mut(), &refs, path)
    }

    /// The index of the watch with this id, if it exists.
    pub fn watch_index(&self, id: &str) -> Option<usize> {
        self.doc
            .get("watches")?
            .as_array_of_tables()?
            .iter()
            .position(|t| t.get("id").and_then(|i| i.as_str()) == Some(id))
    }

    /// Every watch id, in document order.
    pub fn watch_ids(&self) -> Vec<String> {
        self.doc
            .get("watches")
            .and_then(|w| w.as_array_of_tables())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("id").and_then(|i| i.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Append a watch to `[[watches]]`, creating the array if it is absent.
    ///
    /// The new table is appended at the end of the document. That is valid TOML
    /// and leaves every existing byte untouched, which matters more than where
    /// it lands: a reflow to "tidy" the position would rewrite the file.
    pub fn add_watch(&mut self, watch: &Watch) -> Result<(), EditError> {
        let table = watch_to_table(watch)?;
        let entry = self
            .doc
            .entry("watches")
            .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        let array = entry.as_array_of_tables_mut().ok_or(EditError::NoWatches)?;
        array.push(table);
        Ok(())
    }

    /// Remove a watch by id. Returns whether one was removed.
    pub fn remove_watch(&mut self, id: &str) -> Result<bool, EditError> {
        let Some(index) = self.watch_index(id) else {
            return Ok(false);
        };
        let array = self
            .doc
            .get_mut("watches")
            .and_then(|w| w.as_array_of_tables_mut())
            .ok_or(EditError::NoWatches)?;
        array.remove(index);
        Ok(true)
    }
}

/// Render a [`Watch`] as a standalone TOML table whose sub-tables are all
/// inline.
///
/// ⛔ A nested `[watches.poll]` header cannot be appended: toml_edit renders a
/// sub-table at the position of the array-of-tables it belongs to, so the new
/// watch's `[watches.poll]` lands in the middle of the *first* watch and the
/// file reflows. Inlining `poll = { .. }` keeps the appended entry to one
/// header, which is also how the shipped example writes it.
fn watch_to_table(watch: &Watch) -> Result<Table, EditError> {
    let rendered = toml::to_string(watch).map_err(|e| EditError::Serialize(e.to_string()))?;
    let doc = rendered
        .parse::<DocumentMut>()
        .map_err(|e| EditError::Serialize(e.to_string()))?;
    let mut table = doc.as_table().clone();
    inline_sub_tables(&mut table);
    table.set_implicit(false);
    Ok(table)
}

/// Replace every sub-table with an equivalent inline table, in place.
fn inline_sub_tables(table: &mut Table) {
    let keys: Vec<String> = table
        .iter()
        .filter(|(_, item)| item.is_table())
        .map(|(k, _)| k.to_string())
        .collect();
    for key in keys {
        let Some(item) = table.remove(&key) else {
            continue;
        };
        let Item::Table(mut sub) = item else { continue };
        inline_sub_tables(&mut sub);
        if let Some(inline) = sub.into_inline_table().into() {
            table.insert(&key, Item::Value(Value::InlineTable(inline)));
        }
    }
}

fn set_in_table(
    table: &mut Table,
    segments: &[&str],
    value: Value,
    path: &str,
    is_root: bool,
) -> Result<(), EditError> {
    let (head, rest) = split(segments, path)?;
    if rest.is_empty() {
        // Preserve the existing decor (a trailing comment on the line) when the
        // key is already there.
        if let Some(existing) = table.get_mut(head)
            && let Item::Value(old) = existing
        {
            let decor = old.decor().clone();
            let mut new = value;
            *new.decor_mut() = decor;
            *existing = Item::Value(new);
            return Ok(());
        }
        table.insert(head, Item::Value(value));
        // A table that was only implied by its children now holds a value, so
        // it has a header whether we say so or not; say so, and the check below
        // stops treating it as header-less.
        table.set_implicit(false);
        return Ok(());
    }
    // ⛔ M14. A missing intermediate is created inline — `poll = { .. }` inside
    // a watch — EXCEPT under a table that has no header of its own: the root,
    // or `accounts` when the file only ever writes `[accounts.gitlab]`. A value
    // there forces toml_edit to print the header, and it prints it where the
    // parent sits, which for both is above the file's opening comment block:
    // "Add account" put `[accounts]` and the new account on line 1 and pushed
    // the file's own introduction underneath. A standard table instead renders
    // after its last sibling, which is where a person would have typed it.
    let header_less = is_root || table.is_implicit();
    let entry = table.entry(head).or_insert_with(|| {
        if header_less {
            let mut sub = Table::new();
            sub.set_implicit(true);
            sub.decor_mut().set_prefix("\n");
            Item::Table(sub)
        } else {
            Item::Value(Value::InlineTable(InlineTable::new()))
        }
    });
    set_in_item(entry, rest, value, path, head)
}

fn set_in_item(
    item: &mut Item,
    segments: &[&str],
    value: Value,
    path: &str,
    so_far: &str,
) -> Result<(), EditError> {
    match item {
        Item::Table(t) => set_in_table(t, segments, value, path, false),
        Item::Value(Value::InlineTable(t)) => set_in_inline(t, segments, value, path),
        Item::ArrayOfTables(a) => {
            let (head, rest) = split(segments, path)?;
            let index: usize = head.parse().map_err(|_| EditError::NotATable {
                path: so_far.to_string(),
                segment: head.to_string(),
            })?;
            let t = a.get_mut(index).ok_or_else(|| EditError::NoSuchKey {
                path: format!("{so_far}.{head}"),
            })?;
            if rest.is_empty() {
                return Err(EditError::NotATable {
                    path: path.to_string(),
                    segment: head.to_string(),
                });
            }
            set_in_table(t, rest, value, path, false)
        }
        _ => Err(EditError::NotATable {
            path: so_far.to_string(),
            segment: segments.first().unwrap_or(&"").to_string(),
        }),
    }
}

fn set_in_inline(
    table: &mut InlineTable,
    segments: &[&str],
    value: Value,
    path: &str,
) -> Result<(), EditError> {
    let (head, rest) = split(segments, path)?;
    if rest.is_empty() {
        if let Some(old) = table.get_mut(head) {
            let decor = old.decor().clone();
            let mut new = value;
            *new.decor_mut() = decor;
            *old = new;
        } else {
            // ⚠ The last value of `{ max_rows = 3 }` carries the space before
            // the closing brace as its own suffix, and toml_edit keeps it when a
            // key is appended, so the line came out `{ max_rows = 3 , jobs = .. }`.
            // The trailing whitespace moves to the new last value instead.
            let trailing = table.iter_mut().last().and_then(|(_, last)| {
                let suffix = last.decor().suffix()?.as_str()?.to_string();
                if !suffix.trim().is_empty() {
                    return None;
                }
                last.decor_mut().set_suffix("");
                Some(suffix)
            });
            table.insert(head, value);
            if let Some(suffix) = trailing
                && let Some(new) = table.get_mut(head)
            {
                new.decor_mut().set_suffix(suffix);
            }
        }
        return Ok(());
    }
    let entry = table
        .entry(head)
        .or_insert(Value::InlineTable(InlineTable::new()));
    match entry {
        Value::InlineTable(t) => set_in_inline(t, rest, value, path),
        _ => Err(EditError::NotATable {
            path: path.to_string(),
            segment: head.to_string(),
        }),
    }
}

fn unset_in_table(table: &mut Table, segments: &[&str], path: &str) -> Result<bool, EditError> {
    let (head, rest) = split(segments, path)?;
    if rest.is_empty() {
        return Ok(table.remove(head).is_some());
    }
    match table.get_mut(head) {
        Some(Item::Table(t)) => unset_in_table(t, rest, path),
        Some(Item::Value(Value::InlineTable(t))) => unset_in_inline(t, rest, path),
        Some(Item::ArrayOfTables(a)) => {
            let (idx, tail) = split(rest, path)?;
            let Ok(index) = idx.parse::<usize>() else {
                return Ok(false);
            };
            let Some(t) = a.get_mut(index) else {
                return Ok(false);
            };
            if tail.is_empty() {
                a.remove(index);
                return Ok(true);
            }
            unset_in_table(t, tail, path)
        }
        _ => Ok(false),
    }
}

fn unset_in_inline(
    table: &mut InlineTable,
    segments: &[&str],
    path: &str,
) -> Result<bool, EditError> {
    let (head, rest) = split(segments, path)?;
    if rest.is_empty() {
        return Ok(table.remove(head).is_some());
    }
    match table.get_mut(head) {
        Some(Value::InlineTable(t)) => unset_in_inline(t, rest, path),
        _ => Ok(false),
    }
}

fn split<'a>(segments: &'a [&'a str], path: &str) -> Result<(&'a str, &'a [&'a str]), EditError> {
    segments
        .split_first()
        .map(|(h, r)| (*h, r))
        .ok_or_else(|| EditError::NoSuchKey {
            path: path.to_string(),
        })
}

/// Split a dotted path into segments, honouring double quotes.
///
/// A job-name pattern is a perfectly legal table key and may contain a dot, so
/// `watches.0.jobs."re:^build\.docs"` has to address it. Quoting is the escape;
/// unquoted dots split as usual.
///
/// ⛔ Inside quotes a backslash escapes the next character, because the settings
/// pane quotes a key with `JSON.stringify` — which writes `\\` for a backslash
/// and `\"` for a quote — and stripping the quotes without undoing those
/// escapes silently corrupts the key it was asked to address. `re:^build\.docs`
/// came back from every GUI save one backslash longer (`re:^build\\.docs`, then
/// `re:^build\\\\.docs`) until the override matched no job at all, and a `"`
/// inside a key left the quoting unbalanced so the REST of the path stopped
/// splitting. It is not only `[watches.jobs]`: account names travel the same
/// road, so `accounts."gitlab.com".token.env` decoded wrongly would write a
/// token source into a table nobody asked for.
pub fn split_path(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = path.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted = !quoted,
            '\\' if quoted => current.push(unescape(&mut chars)),
            '.' if !quoted => out.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    out.push(current);
    out
}

/// Decode one JSON escape, the cursor sitting just after the backslash.
///
/// The named escapes are here because `JSON.stringify` emits them; `\\` and `\"`
/// fall through the catch-all, which is also the safe answer for a backslash
/// before anything JSON does not escape (a regex's `\d` written unquoted by
/// hand, say): the character itself.
fn unescape(chars: &mut std::str::Chars<'_>) -> char {
    match chars.next() {
        Some('n') => '\n',
        Some('t') => '\t',
        Some('r') => '\r',
        Some('b') => '\u{8}',
        Some('f') => '\u{c}',
        Some('u') => {
            let hex: String = chars.by_ref().take(4).collect();
            u32::from_str_radix(&hex, 16)
                .ok()
                .and_then(char::from_u32)
                .unwrap_or('\u{fffd}')
        }
        Some(other) => other,
        // A path that ends in a backslash is malformed; keep the byte rather
        // than dropping it, so the key that fails to resolve is the one written.
        None => '\\',
    }
}

/// Quote a key for use as one segment of a dotted path, the way the settings
/// pane's `JSON.stringify` does.
///
/// The two sides have to agree exactly; this is the Rust half, and
/// [`split_path`] is what undoes it.
pub fn quote_path_segment(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 2);
    out.push('"');
    for ch in key.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // The rest of C0, as `JSON.stringify` writes it: lowercase hex.
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
