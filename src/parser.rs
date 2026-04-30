//! `.env` file parsing and the canonical "nothing to save" comparator
//! (design.md D4).
//!
//! envroll does NOT roll its own `.env` parser — we delegate to `dotenvy`
//! and only own:
//!
//! - [`parse_buf`] / [`parse_path`]: read a `.env` file or buffer into a
//!   `Vec<(String, String)>` keeping the original first-occurrence key order.
//!   Unparseable input maps to [`EnvrollError::ParseError`].
//! - [`as_key_value_map`]: collapse to a `BTreeMap` with later-wins semantics.
//!   This is the canonical "set" used by the comparator.
//! - [`same_kv_set`]: comparator per design.md D4 — order-insensitive,
//!   value byte-exact, comments / blank lines / trailing-newline differences
//!   ignored, inner-value whitespace IS significant.
//! - [`serialize`]: re-emit a `.env` file. Pre-existing keys keep their
//!   original ordering; new keys (from `updates`) append in insertion order.
//!   Values are always emitted with double-quote wrapping and the four
//!   escapes `dotenvy` understands so the output round-trips cleanly.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use crate::errors::EnvrollError;

/// Parse `bytes` as a `.env` file body (UTF-8 only). Preserves first-occurrence
/// key order; later duplicates appear later in the returned vec, and
/// [`as_key_value_map`] collapses them with later-wins.
pub fn parse_buf(bytes: &[u8]) -> Result<Vec<(String, String)>, EnvrollError> {
    parse_reader(bytes)
}

/// Parse the file at `path` as a `.env` body. Equivalent to reading the file
/// and calling [`parse_buf`].
pub fn parse_path(path: &Path) -> Result<Vec<(String, String)>, EnvrollError> {
    let bytes = std::fs::read(path).map_err(EnvrollError::Io)?;
    parse_buf(&bytes)
}

fn parse_reader<R: Read>(reader: R) -> Result<Vec<(String, String)>, EnvrollError> {
    let iter = dotenvy::Iter::new(reader);
    let mut out = Vec::new();
    for item in iter {
        let (k, v) = item.map_err(|e| EnvrollError::ParseError(e.to_string()))?;
        out.push((k, v));
    }
    Ok(out)
}

/// Collapse a parsed sequence to a canonical key→value map. Later assignments
/// of the same key overwrite earlier ones (matches `dotenvy`'s runtime
/// behavior and design.md D4).
pub fn as_key_value_map(parsed: &[(String, String)]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (k, v) in parsed {
        map.insert(k.clone(), v.clone());
    }
    map
}

/// Are these two parsed-and-collapsed key-value sets equivalent for the
/// purpose of "nothing to save" detection?
///
/// Per design.md D4: this is equality on the [`BTreeMap`] view. Key order,
/// comments, blank lines, and trailing-newline differences are already
/// invisible at this layer (they were stripped in parsing). Inner-value
/// whitespace differences ARE preserved as part of the value bytes, so a
/// changed value with the same trim() still counts as a change.
pub fn same_kv_set(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> bool {
    a == b
}

/// Re-emit a `.env` file body with the supplied `updates` applied.
///
/// - Keys that appear in `parsed` keep their original first-occurrence order;
///   later duplicates of the same key are dropped (only the canonical value
///   is emitted at the first position).
/// - Keys in `updates` that are NOT in `parsed` are appended at the end in
///   the order they appear in the slice.
/// - Every emitted value is double-quoted with the four escapes `dotenvy`
///   understands so the output is a strict subset of the supported syntax
///   and round-trips through [`parse_buf`] cleanly.
///
/// This function does NOT preserve comments or blank lines from the original
/// input — envroll commits the canonical key-value content, and design.md D4
/// classifies comment-only edits as "nothing to save". Callers that want to
/// preserve comments verbatim should use `envroll edit`.
pub fn serialize(parsed: &[(String, String)], updates: &[(String, String)]) -> String {
    let mut effective: BTreeMap<String, String> = as_key_value_map(parsed);
    for (k, v) in updates {
        effective.insert(k.clone(), v.clone());
    }

    let mut out = String::new();
    let mut emitted: BTreeMap<String, ()> = BTreeMap::new();

    for (k, _) in parsed {
        if emitted.contains_key(k) {
            continue;
        }
        let v = effective.get(k).cloned().unwrap_or_default();
        push_kv(&mut out, k, &v);
        emitted.insert(k.clone(), ());
    }

    for (k, v) in updates {
        if emitted.contains_key(k) {
            continue;
        }
        push_kv(&mut out, k, v);
        emitted.insert(k.clone(), ());
    }

    out
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<(String, String)> {
        parse_buf(s.as_bytes()).unwrap()
    }

    fn map(s: &str) -> BTreeMap<String, String> {
        as_key_value_map(&parse(s))
    }

    // ---------- parser ----------

    #[test]
    fn parse_preserves_first_occurrence_order() {
        let p = parse("B=2\nA=1\nC=3\n");
        let keys: Vec<&str> = p.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["B", "A", "C"]);
    }

    #[test]
    fn parse_returns_duplicates_in_source_order() {
        let p = parse("A=1\nA=2\n");
        let collected: Vec<(&str, &str)> =
            p.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(collected, vec![("A", "1"), ("A", "2")]);
    }

    #[test]
    fn parse_unparseable_yields_parse_error() {
        // Unterminated double quote — dotenvy rejects.
        let err = parse_buf(b"BAD=\"unterminated\n").unwrap_err();
        assert!(matches!(err, EnvrollError::ParseError(_)));
    }

    #[test]
    fn as_kv_map_later_wins() {
        let m = map("A=1\nA=2\n");
        assert_eq!(m.get("A").map(String::as_str), Some("2"));
    }

    // ---------- comparator (6.5) ----------

    #[test]
    fn comparator_reordered_keys_are_equal() {
        assert!(same_kv_set(&map("A=1\nB=2\n"), &map("B=2\nA=1\n")));
    }

    #[test]
    fn comparator_comment_edits_are_equal() {
        assert!(same_kv_set(
            &map("# header\nA=1\n# trailer\n"),
            &map("A=1\n# different comment\n")
        ));
    }

    #[test]
    fn comparator_trailing_newline_difference_is_equal() {
        assert!(same_kv_set(&map("A=1\n"), &map("A=1")));
    }

    #[test]
    fn comparator_blank_line_difference_is_equal() {
        assert!(same_kv_set(&map("A=1\n\nB=2\n"), &map("A=1\nB=2\n")));
    }

    #[test]
    fn comparator_inner_whitespace_difference_is_change() {
        // Trailing space INSIDE a quoted value DOES count as a change.
        assert!(!same_kv_set(&map("A=\"value\"\n"), &map("A=\"value \"\n")));
    }

    #[test]
    fn comparator_changed_value_is_change() {
        assert!(!same_kv_set(&map("A=1\n"), &map("A=2\n")));
    }

    #[test]
    fn comparator_added_key_is_change() {
        assert!(!same_kv_set(&map("A=1\n"), &map("A=1\nB=2\n")));
    }

    #[test]
    fn comparator_removed_key_is_change() {
        assert!(!same_kv_set(&map("A=1\nB=2\n"), &map("A=1\n")));
    }

    #[test]
    fn comparator_removed_duplicate_with_same_surviving_value_is_equal() {
        // Source had a duplicate; "later wins" makes both maps {A: 2}.
        assert!(same_kv_set(&map("A=1\nA=2\n"), &map("A=2\n")));
    }

    // ---------- serializer round-trip (6.6) ----------

    fn round_trip_kv(input: &str) {
        let parsed = parse(input);
        let map_in = as_key_value_map(&parsed);
        let serialized = serialize(&parsed, &[]);
        let map_out = as_key_value_map(&parse(&serialized));
        assert_eq!(
            map_in, map_out,
            "round-trip mismatch.\nINPUT:  {input:?}\nSERIALIZED: {serialized:?}"
        );
    }

    #[test]
    fn round_trip_simple_values() {
        round_trip_kv("FOO=bar\nBAZ=qux\n");
    }

    #[test]
    fn round_trip_double_quoted_with_spaces() {
        round_trip_kv("DSN=\"postgres://user:pass@host:5432/db\"\n");
    }

    #[test]
    fn round_trip_value_with_escaped_chars() {
        // dotenvy understands \\ \" \$ \n in double-quoted values.
        round_trip_kv("ESCAPED=\"line1\\nline2 with \\\"quotes\\\" and \\$dollar\"\n");
    }

    #[test]
    fn round_trip_export_prefix_drops_export() {
        // dotenvy parses both `export FOO=bar` and `FOO=bar` to the same KV.
        // We don't preserve the `export` prefix on emit; the round-trip on
        // the parsed map must still match.
        round_trip_kv("export FOO=bar\nBAZ=qux\n");
    }

    #[test]
    fn round_trip_empty_value() {
        round_trip_kv("EMPTY=\nFOO=bar\n");
    }

    #[test]
    fn serialize_preserves_first_occurrence_order_of_existing_keys() {
        let parsed = parse("Z=1\nA=2\nM=3\n");
        let s = serialize(&parsed, &[]);
        let lines: Vec<&str> = s.lines().collect();
        // Three lines in the original order — ignore the value-format detail.
        assert!(lines[0].starts_with("Z="));
        assert!(lines[1].starts_with("A="));
        assert!(lines[2].starts_with("M="));
    }

    #[test]
    fn serialize_appends_new_keys_in_update_order() {
        let parsed = parse("EXISTING=1\n");
        let updates = vec![
            ("NEW_B".to_string(), "2".to_string()),
            ("NEW_A".to_string(), "3".to_string()),
        ];
        let s = serialize(&parsed, &updates);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("EXISTING="));
        assert!(lines[1].starts_with("NEW_B="));
        assert!(lines[2].starts_with("NEW_A="));
    }

    #[test]
    fn serialize_overrides_existing_key_in_place() {
        let parsed = parse("FOO=old\nBAR=keep\n");
        let updates = vec![("FOO".to_string(), "new".to_string())];
        let s = serialize(&parsed, &updates);
        let map_after = as_key_value_map(&parse(&s));
        assert_eq!(map_after.get("FOO").map(String::as_str), Some("new"));
        assert_eq!(map_after.get("BAR").map(String::as_str), Some("keep"));
        // FOO must still be in position 0 (in-place override, not appended).
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("FOO="));
        assert!(lines[1].starts_with("BAR="));
    }

    #[test]
    fn serialize_dedupes_duplicate_keys_to_first_position() {
        let parsed = parse("A=1\nB=2\nA=3\n");
        let s = serialize(&parsed, &[]);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2, "duplicate key should collapse");
        assert!(lines[0].starts_with("A="));
        assert!(lines[1].starts_with("B="));
        // The kept value is the later (winning) one.
        let map_after = as_key_value_map(&parse(&s));
        assert_eq!(map_after.get("A").map(String::as_str), Some("3"));
    }
}
