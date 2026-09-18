//! Moving a `[[watches]]` entry up or down without disturbing anything else.
//!
//! ⛔ This is the one place the shell manipulates TOML itself, and it should
//! not stay that way: `ConfigEditor` has `AddWatch` and `RemoveWatch` but no
//! move. The core owning an `Edit::MoveWatch` would delete this file.
//!
//! ⛔ Reordering the array is not enough, and the first version stopped there.
//! `toml_edit` renders a parsed document by each table's POSITION, a number it
//! recorded while parsing, and a cloned table keeps its number; so the array
//! was reordered in memory and the text came out byte-identical, and "Move up"
//! wrote the same file back. The positions have to move with the watches.
//!
//! A watch is not one table either: `[watches.jobs]` and `[watches.notify]`
//! after a `[[watches]]` header are separate tables, with their own positions,
//! that belong to that element. So the rule is: take every position held by a
//! table under ANY watch, sorted; list each watch's own tables in their file
//! order, watches in their NEW order; and hand the sorted positions out along
//! that list. Each watch keeps its block, its sub-tables and its comments (a
//! table's leading comment is its own decor, so it travels with it), and every
//! table that is not under `[[watches]]` keeps its number and so its place.

use std::collections::HashMap;

use toml_edit::{DocumentMut, Item, Table};

/// The text with watch `id` moved by `delta` places, clamped to the ends.
///
/// `Ok(None)` when the move is a no-op (already first or last).
pub fn move_watch(raw: &str, id: &str, delta: i32) -> Result<Option<String>, String> {
    let mut doc = raw.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    let Some(array) = doc
        .get_mut("watches")
        .and_then(|w| w.as_array_of_tables_mut())
    else {
        return Err("the document has no [[watches]] array".into());
    };

    let Some(from) = array
        .iter()
        .position(|t| t.get("id").and_then(|i| i.as_str()) == Some(id))
    else {
        return Err(format!("no watch with id {id:?}"));
    };
    let last = array.len() as i64 - 1;
    let to = (from as i64 + delta as i64).clamp(0, last) as usize;
    if to == from {
        return Ok(None);
    }

    // Each watch's positions, in file order, before anything moves.
    let owned: Vec<Vec<isize>> = array
        .iter()
        .map(|t| {
            let mut positions = Vec::new();
            positions_under(t, &mut positions);
            positions.sort_unstable();
            positions
        })
        .collect();
    let mut slots: Vec<isize> = owned.iter().flatten().copied().collect();
    slots.sort_unstable();

    let mut order: Vec<usize> = (0..owned.len()).collect();
    let moved = order.remove(from);
    order.insert(to, moved);

    let remap: HashMap<isize, isize> = order
        .iter()
        .flat_map(|&w| owned[w].iter().copied())
        .zip(slots.iter().copied())
        .collect();

    let mut tables: Vec<Table> = array.iter().cloned().collect();
    let table = tables.remove(from);
    tables.insert(to, table);
    array.clear();
    for mut table in tables {
        renumber(&mut table, &remap);
        array.push(table);
    }

    Ok(Some(doc.to_string()))
}

/// Every recorded position in `table` and the tables nested under it.
fn positions_under(table: &Table, out: &mut Vec<isize>) {
    if let Some(p) = table.position() {
        out.push(p);
    }
    for (_, item) in table.iter() {
        match item {
            Item::Table(t) => positions_under(t, out),
            Item::ArrayOfTables(a) => a.iter().for_each(|t| positions_under(t, out)),
            _ => {}
        }
    }
}

fn renumber(table: &mut Table, remap: &HashMap<isize, isize>) {
    if let Some(p) = table.position()
        && let Some(&new) = remap.get(&p)
    {
        table.set_position(Some(new));
    }
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(t) => renumber(t, remap),
            Item::ArrayOfTables(a) => a.iter_mut().for_each(|t| renumber(t, remap)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"# bridgewatch
[accounts.gl]
base_url = "https://gitlab.com"

# The one that matters.
[[watches]]
id = "first"
account = "gl"
project = 1

[watches.jobs]
"kics" = "ignore"

# Hourly.
[[watches]]
id = "second"
account = "gl"
project = 2

[watches.notify]
on = ["failed"]

[[watches]]
id = "third"
account = "gl"
project = 3

[ui]
theme_css = ""
"#;

    fn ids(text: &str) -> Vec<String> {
        let doc = text.parse::<DocumentMut>().unwrap();
        doc["watches"]
            .as_array_of_tables()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn moving_a_watch_changes_the_emitted_text() {
        let out = move_watch(DOC, "second", -1).unwrap().expect("a move");
        assert_ne!(out, DOC, "the rendered text is byte-identical");
        assert_eq!(ids(&out), ["second", "first", "third"]);
    }

    #[test]
    fn a_watch_takes_its_sub_tables_and_comments_with_it() {
        let out = move_watch(DOC, "second", -1).unwrap().unwrap();
        let second = out.find("# Hourly.").unwrap();
        let notify = out.find("[watches.notify]").unwrap();
        let first = out.find("# The one that matters.").unwrap();
        let jobs = out.find("[watches.jobs]").unwrap();
        assert!(second < notify && notify < first && first < jobs, "{out}");

        // And the core reads it back with the sub-tables on the right watch.
        let loaded = bridgewatch_core::config::parse_str(&out, std::path::Path::new("x.toml"))
            .expect("the result loads")
            .config;
        assert_eq!(loaded.watches[0].id, "second");
        assert!(loaded.watches[0].jobs.entries().is_empty());
        assert_eq!(loaded.watches[1].id, "first");
        assert_eq!(loaded.watches[1].jobs.entries().len(), 1);
    }

    #[test]
    fn tables_outside_the_watches_stay_where_they_were() {
        let out = move_watch(DOC, "first", 2).unwrap().unwrap();
        assert_eq!(ids(&out), ["second", "third", "first"]);
        assert!(out.starts_with("# bridgewatch\n[accounts.gl]"), "{out}");
        assert!(out.trim_end().ends_with("[ui]\ntheme_css = \"\""), "{out}");
    }

    #[test]
    fn a_move_past_either_end_is_a_no_op() {
        assert_eq!(move_watch(DOC, "first", -1).unwrap(), None);
        assert_eq!(move_watch(DOC, "third", 5).unwrap(), None);
        assert!(move_watch(DOC, "nope", 1).is_err());
    }

    #[test]
    fn moving_down_and_back_up_restores_the_file() {
        let down = move_watch(DOC, "first", 1).unwrap().unwrap();
        let back = move_watch(&down, "first", -1).unwrap().unwrap();
        assert_eq!(back, DOC);
    }
}
