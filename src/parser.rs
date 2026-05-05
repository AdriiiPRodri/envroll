//! `.env` file parsing utilities.
//!
//! envroll delegates the heavy lifting to `dotenvy`, but applies a thin
//! tolerance pre-pass on read so it can ingest the unquoted multi-word
//! values that python-dotenv / Django-style `.env` files routinely contain
//! (e.g. `DEFAULT_FROM_EMAIL=Display Name <a@b.com>`). The serializer
//! always re-emits canonical `KEY="value"` form, so the in-vault bytes are
//! a strict subset of what dotenvy parses.
//!
//! Owned pieces:
//!
//! - [`parse_buf`] / [`parse_path`]: read a `.env` file or buffer into a
//!   `Vec<(String, String)>` keeping the original first-occurrence key order.
//!   Unparseable input maps to [`EnvrollError::ParseError`].
//! - [`normalize_for_dotenvy`]: pre-pass that auto-quotes unquoted
//!   multi-word values so dotenvy stops rejecting them. Lines already inside
//!   a multi-line quoted block are passed through untouched.
//! - [`as_key_value_map`]: collapse to a `BTreeMap` with later-wins semantics.
//!   Used by `status`, `diff`, `get`, `exec`, and friends to project a `.env`
//!   into its canonical key-value set.
//! - [`same_kv_set`]: order-insensitive equality on the `BTreeMap` view
//!   (comments, blank lines, key order, trailing-newline differences are
//!   ignored; inner-value whitespace IS significant). NOT used by `save` —
//!   `save` compares raw bytes against the decrypted tip so cosmetic edits
//!   (comments, key reordering) get persisted end-to-end.
//! - [`serialize`]: re-emit a `.env` file. Pre-existing keys keep their
//!   original ordering; new keys (from `updates`) append in insertion order.
//!   Values are always emitted with double-quote wrapping and the four
//!   escapes `dotenvy` understands so the output round-trips cleanly.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;

use crate::errors::EnvrollError;

/// Parse `bytes` as a `.env` file body (UTF-8 only). Preserves first-occurrence
/// key order; later duplicates appear later in the returned vec, and
/// [`as_key_value_map`] collapses them with later-wins.
pub fn parse_buf(bytes: &[u8]) -> Result<Vec<(String, String)>, EnvrollError> {
    let normalized = normalize_for_dotenvy(bytes);
    parse_dotenvy(normalized.as_ref())
}

/// Parse the file at `path` as a `.env` body. Equivalent to reading the file
/// and calling [`parse_buf`].
pub fn parse_path(path: &Path) -> Result<Vec<(String, String)>, EnvrollError> {
    let bytes = std::fs::read(path).map_err(EnvrollError::Io)?;
    parse_buf(&bytes)
}

fn parse_dotenvy(bytes: &[u8]) -> Result<Vec<(String, String)>, EnvrollError> {
    let iter = dotenvy::Iter::new(bytes);
    let mut out = Vec::new();
    for item in iter {
        let (k, v) = item.map_err(|e| EnvrollError::ParseError(e.to_string()))?;
        out.push((k, v));
    }
    Ok(out)
}

/// Tolerance pre-pass: rewrite unquoted multi-word values as double-quoted
/// values so dotenvy accepts them.
///
/// dotenvy follows POSIX-shell-like rules in unquoted values: once it sees
/// whitespace, the next non-whitespace, non-`#` character is a parse error.
/// That excludes shapes like `KEY=Display Name <a@b.com>` that python-dotenv
/// and Django accept. This pass detects such lines and wraps the value in
/// `"..."` with the four standard escapes dotenvy understands (`\\`, `\"`,
/// `\$`, `\n`), so the file becomes a strict subset of dotenvy syntax.
///
/// Lines that already start with a quote, are comments, are inside an open
/// multi-line quoted value, or are blank are passed through verbatim.
pub fn normalize_for_dotenvy(input: &[u8]) -> Cow<'_, [u8]> {
    let s = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(_) => return Cow::Borrowed(input),
    };

    let mut out: Option<String> = None;
    let mut quote_state = QuoteState::Closed;

    for (line_start, line) in physical_lines(s) {
        let rewritten = if matches!(quote_state, QuoteState::Closed) {
            try_quote_unquoted_value(line)
        } else {
            None
        };

        if let Some(new_line) = rewritten {
            if out.is_none() {
                out = Some(s[..line_start].to_string());
            }
            out.as_mut().unwrap().push_str(&new_line);
            // Emitted line is fully self-contained; quote state remains Closed.
        } else {
            if let Some(o) = out.as_mut() {
                o.push_str(line);
            }
            quote_state = update_quote_state(quote_state, line);
        }
    }

    match out {
        Some(s) => Cow::Owned(s.into_bytes()),
        None => Cow::Borrowed(input),
    }
}

fn physical_lines(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut start = 0usize;
    std::iter::from_fn(move || {
        if start >= s.len() {
            return None;
        }
        let rel = s[start..].find('\n');
        let (line_start, end) = match rel {
            Some(r) => (start, start + r + 1),
            None => (start, s.len()),
        };
        let line = &s[line_start..end];
        start = end;
        Some((line_start, line))
    })
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum QuoteState {
    Closed,
    InWeakQuote,
    InWeakQuoteEscape,
    InStrongQuote,
}

fn update_quote_state(mut state: QuoteState, line: &str) -> QuoteState {
    for c in line.chars() {
        state = match state {
            QuoteState::Closed => match c {
                '"' => QuoteState::InWeakQuote,
                '\'' => QuoteState::InStrongQuote,
                '#' => return QuoteState::Closed,
                _ => QuoteState::Closed,
            },
            QuoteState::InWeakQuote => match c {
                '\\' => QuoteState::InWeakQuoteEscape,
                '"' => QuoteState::Closed,
                _ => QuoteState::InWeakQuote,
            },
            QuoteState::InWeakQuoteEscape => QuoteState::InWeakQuote,
            QuoteState::InStrongQuote => match c {
                '\'' => QuoteState::Closed,
                _ => QuoteState::InStrongQuote,
            },
        };
    }
    state
}

/// Try to rewrite a single physical line `KEY=unquoted value` as
/// `KEY="unquoted value"`. Returns `None` if the line doesn't need rewriting
/// (already quoted, comment, blank, or the value has no internal whitespace).
fn try_quote_unquoted_value(line: &str) -> Option<String> {
    let (body, line_ending) = strip_line_ending(line);

    let trimmed = body.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let leading_ws = &body[..body.len() - trimmed.len()];

    let (rest, had_export) = match strip_export_prefix(trimmed) {
        Some(after) => (after, true),
        None => (trimmed, false),
    };

    if !rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    let key_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(rest.len());
    if key_end == 0 {
        return None;
    }
    let key = &rest[..key_end];
    let after_key = rest[key_end..].trim_start_matches([' ', '\t']);
    let after_eq = after_key.strip_prefix('=')?;
    let value_region = after_eq.trim_start_matches([' ', '\t']);

    if value_region.is_empty() {
        return None;
    }
    if value_region.starts_with('"') || value_region.starts_with('\'') {
        return None;
    }

    let (value_raw, trailing_comment) = split_trailing_comment(value_region);
    let value = value_raw.trim_end_matches([' ', '\t']);

    if value.is_empty() {
        return None;
    }
    if !needs_quoting(value) {
        return None;
    }

    let mut out = String::with_capacity(line.len() + 8);
    out.push_str(leading_ws);
    if had_export {
        out.push_str("export ");
    }
    out.push_str(key);
    out.push('=');
    out.push('"');
    push_double_quote_escaped(&mut out, value);
    out.push('"');
    if let Some(comment) = trailing_comment {
        out.push(' ');
        out.push_str(comment);
    }
    out.push_str(line_ending);
    Some(out)
}

fn strip_line_ending(line: &str) -> (&str, &str) {
    if let Some(stripped) = line.strip_suffix("\r\n") {
        (stripped, "\r\n")
    } else if let Some(stripped) = line.strip_suffix('\n') {
        (stripped, "\n")
    } else {
        (line, "")
    }
}

fn strip_export_prefix(s: &str) -> Option<&str> {
    let after = s.strip_prefix("export")?;
    let trimmed = after.trim_start_matches([' ', '\t']);
    if trimmed.len() == after.len() {
        // `export` not followed by whitespace — it's just a key named `exportFOO`.
        None
    } else {
        Some(trimmed)
    }
}

fn split_trailing_comment(s: &str) -> (&str, Option<&str>) {
    let bytes = s.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'#' && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return (&s[..i], Some(&s[i..]));
        }
    }
    (s, None)
}

fn needs_quoting(value: &str) -> bool {
    value.chars().any(|c| c == ' ' || c == '\t')
}

fn push_double_quote_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            other => out.push(other),
        }
    }
}

/// Collapse a parsed sequence to a canonical key→value map. Later assignments
/// of the same key overwrite earlier ones (matches `dotenvy`'s runtime
/// behavior).
pub fn as_key_value_map(parsed: &[(String, String)]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (k, v) in parsed {
        map.insert(k.clone(), v.clone());
    }
    map
}

/// Order-insensitive equality on two parsed-and-collapsed key-value sets.
///
/// This is equality on the [`BTreeMap`] view. Key order, comments, blank
/// lines, and trailing-newline differences are already invisible at this
/// layer (they were stripped in parsing). Inner-value whitespace differences
/// ARE preserved as part of the value bytes, so a changed value with the
/// same trim() still counts as a change.
///
/// `save` does NOT use this comparator — it compares raw bytes so that
/// cosmetic edits (comment additions, key reordering, blank-line tweaks)
/// produce a real commit instead of being silently dropped. Other call
/// sites that genuinely care only about the KV projection are free to use
/// this.
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
/// input. Callers that mutate values via the canonical-form pipeline (`set`,
/// `rename-key`, `copy`) therefore drop comments by construction. Callers that
/// want to preserve comments verbatim should use `envroll edit` (free-form
/// editor invocation) or `envroll save` (which writes the working copy bytes
/// as-is and persists comment-only changes too).
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

    // ---------- tolerance pre-pass: unquoted multi-word values ----------

    #[test]
    fn parse_unquoted_value_with_spaces_is_accepted() {
        let p =
            parse("DEFAULT_FROM_EMAIL=Prowler Alerts <postmaster@sandbox.mailgun.org>\nOTHER=ok\n");
        assert_eq!(
            p,
            vec![
                (
                    "DEFAULT_FROM_EMAIL".to_string(),
                    "Prowler Alerts <postmaster@sandbox.mailgun.org>".to_string()
                ),
                ("OTHER".to_string(), "ok".to_string()),
            ]
        );
    }

    #[test]
    fn parse_unquoted_value_with_inline_comment_is_accepted() {
        let p = parse("EMAIL=Display Name <a@b.com> # legacy alias\n");
        assert_eq!(
            p,
            vec![("EMAIL".to_string(), "Display Name <a@b.com>".to_string())]
        );
    }

    #[test]
    fn parse_unquoted_value_preserves_inner_dollar_literally() {
        // `$` inside an unquoted-with-spaces value must NOT trigger
        // substitution after we wrap it — the user wrote a literal `$`.
        let p = parse("MSG=Price is $10 today\n");
        assert_eq!(
            p,
            vec![("MSG".to_string(), "Price is $10 today".to_string())]
        );
    }

    #[test]
    fn parse_unquoted_value_preserves_inner_backslash_literally() {
        let p = parse("PATH_HINT=C:\\Program Files\\App\n");
        assert_eq!(
            p,
            vec![(
                "PATH_HINT".to_string(),
                "C:\\Program Files\\App".to_string()
            )]
        );
    }

    #[test]
    fn parse_unquoted_value_preserves_inner_double_quote() {
        let p = parse("Q=he said \"hi\" loudly\n");
        assert_eq!(
            p,
            vec![("Q".to_string(), "he said \"hi\" loudly".to_string())]
        );
    }

    #[test]
    fn parse_export_prefix_with_unquoted_multi_word_value_is_accepted() {
        let p = parse("export GREETING=hello world\n");
        assert_eq!(p, vec![("GREETING".to_string(), "hello world".to_string())]);
    }

    #[test]
    fn parse_already_quoted_multi_word_is_unchanged() {
        let p = parse("KEY=\"hello world\"\n");
        assert_eq!(p, vec![("KEY".to_string(), "hello world".to_string())]);
    }

    #[test]
    fn parse_strong_quoted_multi_word_is_unchanged() {
        let p = parse("KEY='hello world'\n");
        assert_eq!(p, vec![("KEY".to_string(), "hello world".to_string())]);
    }

    #[test]
    fn parse_multi_line_double_quoted_value_is_not_corrupted_by_normalizer() {
        // The continuation line happens to start with `KEY=` shape, but it
        // is INSIDE an open weak quote — the normalizer must leave it alone.
        let p = parse("FOO=\"line one\nKEY=part of foo\"\nBAR=baz\n");
        assert_eq!(
            p,
            vec![
                ("FOO".to_string(), "line one\nKEY=part of foo".to_string()),
                ("BAR".to_string(), "baz".to_string()),
            ]
        );
    }

    #[test]
    fn parse_no_trailing_newline_unquoted_multi_word_is_accepted() {
        let p = parse("KEY=hello world");
        assert_eq!(p, vec![("KEY".to_string(), "hello world".to_string())]);
    }

    #[test]
    fn parse_single_word_unquoted_value_unchanged() {
        // Sanity: lines that don't need quoting must be left alone byte-wise
        // by the normalizer (covered indirectly via parse-equivalence).
        let p = parse("KEY=value\n");
        assert_eq!(p, vec![("KEY".to_string(), "value".to_string())]);
    }

    #[test]
    fn parse_comment_only_line_unchanged() {
        let p = parse("# KEY=looks like an assignment but isn't\nREAL=ok\n");
        assert_eq!(p, vec![("REAL".to_string(), "ok".to_string())]);
    }

    #[test]
    fn normalize_passthrough_when_nothing_to_change() {
        // No rewriting needed → must return Borrowed (zero-copy hot path).
        let input = b"A=1\nB=2\n";
        let n = normalize_for_dotenvy(input);
        assert!(matches!(n, Cow::Borrowed(_)));
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
