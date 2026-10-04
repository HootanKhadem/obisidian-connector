//! Pure, in-memory edits to a note's markdown: text replacement, heading sections and frontmatter.
//!
//! Each function takes the current content and returns the new content, or a reason the edit
//! cannot be applied. Nothing here touches the disk.

use serde_json::{Map, Value};

use crate::markdown::{parse_scalar, split_frontmatter, unquote};

/// Where to insert content relative to a note or a heading section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Start,
    End,
}

/// Byte offsets of a heading section: its heading line and the body that follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub heading_start: usize,
    pub body_start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Heading {
    level: usize,
    text: String,
    start: usize,
    body_start: usize,
}

/// Replaces `old` with `new`. `old` must occur exactly once unless `replace_all` is set.
pub fn replace_text(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<(String, usize), String> {
    if old.is_empty() {
        return Err("old_text must not be empty".into());
    }
    let count = content.matches(old).count();
    if count == 0 {
        return Err("old_text was not found in the note".into());
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_text occurs {count} times; include more surrounding text to make it unique, or set replace_all"
        ));
    }
    Ok((content.replace(old, new), count))
}

/// Inserts `text` at the start or end of the note, or of the section under `heading`.
///
/// The start of the note is just after its frontmatter. The end of a section is just after its
/// last non-blank line, so the blank line before the next heading is kept.
pub fn insert(
    content: &str,
    text: &str,
    heading: Option<&str>,
    position: Position,
) -> Result<String, String> {
    let at = match (heading, position) {
        (None, Position::Start) => content.len() - split_frontmatter(content).1.len(),
        (None, Position::End) => content.len(),
        (Some(heading), Position::Start) => find_section(content, heading)?.body_start,
        (Some(heading), Position::End) => {
            let section = find_section(content, heading)?;
            let body = &content[section.body_start..section.end];
            let last_text = body.trim_end().len();
            if last_text == 0 {
                section.body_start
            } else {
                let after = section.body_start + last_text;
                content[after..section.end]
                    .find('\n')
                    .map_or(section.end, |newline| after + newline + 1)
            }
        }
    };
    Ok(insert_at(content, at, text))
}

/// Replaces the body of the section under `heading`, keeping the heading line itself.
pub fn replace_section(content: &str, heading: &str, text: &str) -> Result<String, String> {
    let section = find_section(content, heading)?;
    let body = &content[section.body_start..section.end];
    let trailing = &body[body.trim_end().len()..];
    let rest = &content[section.end..];
    let mut out = content[..section.body_start].to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    let text = text.trim_end_matches(['\r', '\n']);
    if !text.is_empty() {
        out.push_str(text);
        out.push_str(if trailing.is_empty() && !rest.is_empty() {
            "\n"
        } else {
            trailing
        });
    } else if !rest.is_empty() {
        out.push('\n');
    }
    out.push_str(rest);
    Ok(out)
}

/// Returns the section under `heading`, including its heading line.
pub fn read_section<'a>(content: &'a str, heading: &str) -> Result<&'a str, String> {
    let section = find_section(content, heading)?;
    Ok(&content[section.heading_start..section.end])
}

/// Locates a heading section.
///
/// `heading` is matched case-insensitively against the heading text. Leading `#`s restrict the
/// level (`## Tasks`), and `Parent > Child` picks a heading nested under another one.
pub fn find_section(content: &str, heading: &str) -> Result<Section, String> {
    let headings = headings(content);
    let mut range = (0, content.len());
    let mut parent_level = 0;
    let mut found = None;
    for part in heading.split('>') {
        let part = part.trim();
        let wanted_text = part.trim_start_matches('#').trim();
        let wanted_level = part.len() - part.trim_start_matches('#').len();
        if wanted_text.is_empty() {
            return Err(format!("invalid heading '{heading}'"));
        }
        let mut matches: Vec<(usize, &Heading)> = headings
            .iter()
            .enumerate()
            // Inside a parent's range every heading is already deeper than the parent, because
            // the range ends at the next heading of the parent's level or higher.
            .filter(|(_, candidate)| {
                (range.0..range.1).contains(&candidate.start)
                    && (wanted_level == 0 || candidate.level == wanted_level)
                    && candidate.text.to_lowercase() == wanted_text.to_lowercase()
            })
            .collect();
        if parent_level > 0 {
            // Under a parent, the closest level wins: `Day > Notes` means Day's own Notes.
            let closest = matches.iter().map(|(_, candidate)| candidate.level).min();
            matches.retain(|(_, candidate)| Some(candidate.level) == closest);
        }
        let (index, matched) = match matches.as_slice() {
            [] => return Err(format!("heading '{part}' was not found in the note")),
            [single] => *single,
            many => {
                return Err(format!(
                    "heading '{part}' matches {} headings; add its level (e.g. '## {wanted_text}') or its parent (e.g. 'Parent > {wanted_text}')",
                    many.len()
                ));
            }
        };
        let end = headings[index + 1..]
            .iter()
            .find(|next| next.level <= matched.level)
            .map_or(content.len(), |next| next.start);
        range = (matched.body_start, end);
        parent_level = matched.level;
        found = Some(Section {
            heading_start: matched.start,
            body_start: matched.body_start,
            end,
        });
    }
    found.ok_or_else(|| format!("invalid heading '{heading}'"))
}

/// The ATX headings (`# Title`) of a note, skipping its frontmatter and fenced code blocks.
fn headings(content: &str) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut offset = content.len() - split_frontmatter(content).1.len();
    let mut in_fence = false;
    for line in content[offset..].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.len() - trimmed.len() > 3 {
            continue;
        }
        let level = trimmed.len() - trimmed.trim_start_matches('#').len();
        let after_hashes = &trimmed[level..];
        if (1..=6).contains(&level)
            && (after_hashes.trim().is_empty() || after_hashes.starts_with([' ', '\t']))
        {
            headings.push(Heading {
                level,
                text: after_hashes.trim().trim_end_matches('#').trim().to_string(),
                start,
                body_start: offset,
            });
        }
    }
    headings
}

/// Inserts `text` at a byte offset, keeping it on lines of its own.
fn insert_at(content: &str, at: usize, text: &str) -> String {
    let (before, after) = content.split_at(at);
    let mut out = String::with_capacity(content.len() + text.len() + 2);
    out.push_str(before);
    if !before.is_empty() && !before.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(text);
    if !after.is_empty() && !text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(after);
    out
}

/// Sets frontmatter properties, leaving the others and the note body untouched.
///
/// A `null` value removes the property. Frontmatter is created when the note has none, and
/// removed when the last property is.
pub fn update_frontmatter(content: &str, properties: &Map<String, Value>) -> String {
    let (yaml, body) = split_frontmatter(content);
    // Each block is a top-level `key:` line plus its indented or `- item` continuation lines.
    let mut blocks: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for line in yaml.unwrap_or_default().lines() {
        let trimmed = line.trim();
        let continues = line.starts_with(char::is_whitespace)
            || trimmed.starts_with("- ")
            || trimmed == "-"
            || trimmed.is_empty();
        match (continues, blocks.last_mut(), line.split_once(':')) {
            (true, Some(block), _) => block.1.push(line.to_string()),
            (false, _, Some((key, _))) if !trimmed.starts_with('#') => blocks.push((
                Some(unquote(key.trim()).to_string()),
                vec![line.to_string()],
            )),
            _ => blocks.push((None, vec![line.to_string()])),
        }
    }
    for (key, value) in properties {
        let position = blocks
            .iter()
            .position(|(existing, _)| existing.as_deref() == Some(key.as_str()));
        match (position, value) {
            (Some(index), Value::Null) => {
                blocks.remove(index);
            }
            (None, Value::Null) => {}
            (Some(index), value) => blocks[index].1 = render_property(key, value),
            (None, value) => blocks.push((Some(key.clone()), render_property(key, value))),
        }
    }
    let lines: Vec<String> = blocks.into_iter().flat_map(|(_, lines)| lines).collect();
    if lines.iter().all(|line| line.trim().is_empty()) {
        return body.to_string();
    }
    format!("---\n{}\n---\n{body}", lines.join("\n"))
}

fn render_property(key: &str, value: &Value) -> Vec<String> {
    let key = if key.contains(':') || key.starts_with(['"', '\'', '#', '-', ' ']) {
        Value::String(key.to_string()).to_string()
    } else {
        key.to_string()
    };
    match value {
        Value::Array(items) if items.is_empty() => vec![format!("{key}: []")],
        Value::Array(items) => std::iter::once(format!("{key}:"))
            .chain(
                items
                    .iter()
                    .map(|item| format!("  - {}", render_scalar(item))),
            )
            .collect(),
        value => vec![format!("{key}: {}", render_scalar(value))],
    }
}

/// Renders a value as YAML, quoting strings that would otherwise read back as something else.
fn render_scalar(value: &Value) -> String {
    match value {
        Value::String(text) => {
            let plain = parse_scalar(text) == *value
                && text.trim() == text
                && !text.contains(['\n', '\r', '\t'])
                && !text.contains(": ")
                && !text.contains(" #")
                && !text.ends_with(':')
                && !text.starts_with([
                    '[', ']', '{', '}', '>', '|', '*', '&', '!', '%', '@', '`', '"', '\'', '-',
                    '?', ',', '#',
                ]);
            if plain {
                text.clone()
            } else {
                value.to_string()
            }
        }
        // JSON is valid YAML flow syntax for numbers, booleans and nested values.
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse_frontmatter;
    use serde_json::json;

    const NOTE: &str =
        "---\ntitle: Plan\n---\n# Plan\nIntro\n\n## Tasks\n- [ ] one\n\n## Notes\nSome notes\n";

    #[test]
    fn replace_text_replaces_a_unique_match() {
        let (out, count) = replace_text("a b c", "b", "B", false).unwrap();
        assert_eq!((out.as_str(), count), ("a B c", 1));
    }

    #[test]
    fn replace_text_rejects_missing_empty_and_ambiguous_text() {
        assert!(
            replace_text("abc", "", "x", false)
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            replace_text("abc", "z", "x", false)
                .unwrap_err()
                .contains("not found")
        );
        let error = replace_text("a a", "a", "b", false).unwrap_err();
        assert!(error.contains("2 times"), "{error}");
    }

    #[test]
    fn replace_text_can_replace_all() {
        assert_eq!(
            replace_text("a a a", "a", "b", true).unwrap(),
            ("b b b".to_string(), 3)
        );
    }

    #[test]
    fn insert_at_note_start_goes_after_frontmatter() {
        let out = insert(NOTE, "Top", None, Position::Start).unwrap();
        assert!(
            out.starts_with("---\ntitle: Plan\n---\nTop\n# Plan\n"),
            "{out}"
        );
        assert_eq!(
            insert("body", "Top", None, Position::Start).unwrap(),
            "Top\nbody"
        );
    }

    #[test]
    fn insert_at_note_end_appends_on_a_new_line() {
        assert_eq!(insert("a", "b", None, Position::End).unwrap(), "a\nb");
        assert_eq!(insert("a\n", "b", None, Position::End).unwrap(), "a\nb");
        assert_eq!(insert("", "b", None, Position::End).unwrap(), "b");
    }

    #[test]
    fn insert_at_section_end_keeps_the_blank_line_before_the_next_heading() {
        let out = insert(NOTE, "- [ ] two", Some("Tasks"), Position::End).unwrap();
        assert!(
            out.contains("## Tasks\n- [ ] one\n- [ ] two\n\n## Notes\n"),
            "{out}"
        );
    }

    #[test]
    fn insert_at_section_start_goes_right_after_the_heading() {
        let out = insert(NOTE, "- [ ] zero", Some("## tasks"), Position::Start).unwrap();
        assert!(out.contains("## Tasks\n- [ ] zero\n- [ ] one\n"), "{out}");
    }

    #[test]
    fn insert_into_last_or_empty_sections() {
        let out = insert(NOTE, "More", Some("Notes"), Position::End).unwrap();
        assert!(out.ends_with("## Notes\nSome notes\nMore"), "{out}");
        assert_eq!(
            insert("## A\n## B\n", "x", Some("A"), Position::End).unwrap(),
            "## A\nx\n## B\n"
        );
        assert_eq!(
            insert("## A", "x", Some("A"), Position::Start).unwrap(),
            "## A\nx"
        );
        assert_eq!(
            insert("## A\ntext", "x", Some("A"), Position::End).unwrap(),
            "## A\ntext\nx"
        );
    }

    #[test]
    fn insert_at_section_end_keeps_trailing_spaces_on_the_last_line() {
        // Two trailing spaces are a markdown line break, so they belong to the line before.
        let note = "## A\ntext  \n\n## B\n";
        assert_eq!(
            insert(note, "x", Some("A"), Position::End).unwrap(),
            "## A\ntext  \nx\n\n## B\n"
        );
    }

    #[test]
    fn a_section_includes_its_subsections() {
        let note = "# A\nintro\n## B\nb text\n# C\nc text\n";
        let out = insert(note, "end of A", Some("A"), Position::End).unwrap();
        assert_eq!(out, "# A\nintro\n## B\nb text\nend of A\n# C\nc text\n");
    }

    #[test]
    fn find_section_disambiguates_by_level_and_parent() {
        let note = "# Mon\n## Notes\nmon\n# Tue\n## Notes\ntue\n### Notes\ndeep\n";
        let error = find_section(note, "Notes").unwrap_err();
        assert!(error.contains("matches 3 headings"), "{error}");
        assert!(
            find_section(note, "## Notes")
                .unwrap_err()
                .contains("matches 2")
        );
        assert_eq!(
            read_section(note, "Tue > Notes").unwrap(),
            "## Notes\ntue\n### Notes\ndeep\n"
        );
        assert_eq!(
            read_section(note, "Mon > Notes").unwrap(),
            "## Notes\nmon\n"
        );
        assert_eq!(
            read_section(note, "Tue > Notes > Notes").unwrap(),
            "### Notes\ndeep\n"
        );
        assert_eq!(
            read_section(note, "Tue > ### Notes").unwrap(),
            "### Notes\ndeep\n"
        );
        assert!(find_section(note, "Wed").unwrap_err().contains("not found"));
        assert!(
            find_section(note, "Mon > Tue")
                .unwrap_err()
                .contains("not found")
        );
        assert!(find_section(note, "##").unwrap_err().contains("invalid"));
        assert!(
            find_section(note, "Mon > ")
                .unwrap_err()
                .contains("invalid")
        );
    }

    #[test]
    fn headings_ignore_code_blocks_frontmatter_and_non_headings() {
        let note = "---\n# not: heading\n---\n```\n# code\n```\n#tag\n    # indented code\n####### seven\n# Real #\n#\n";
        let found: Vec<_> = headings(note)
            .into_iter()
            .map(|h| (h.level, h.text))
            .collect();
        assert_eq!(found, vec![(1, "Real".to_string()), (1, String::new())]);
    }

    #[test]
    fn headings_may_be_indented_up_to_three_spaces() {
        let note = "   ## Three\nkept\n    ## Four\ncode\n";
        let found: Vec<_> = headings(note)
            .into_iter()
            .map(|heading| (heading.level, heading.text))
            .collect();
        assert_eq!(found, vec![(2, "Three".to_string())]);
    }

    #[test]
    fn replace_section_keeps_heading_and_spacing() {
        let out = replace_section(NOTE, "Tasks", "- [x] done\n").unwrap();
        assert!(
            out.contains("## Tasks\n- [x] done\n\n## Notes\nSome notes\n"),
            "{out}"
        );
        let out = replace_section(NOTE, "Notes", "New").unwrap();
        assert!(out.ends_with("## Notes\nNew\n"), "{out}");
    }

    #[test]
    fn replace_section_handles_empty_bodies_and_text() {
        assert_eq!(
            replace_section("## A\n## B", "A", "x").unwrap(),
            "## A\nx\n## B"
        );
        assert_eq!(replace_section("## A", "A", "x").unwrap(), "## A\nx");
        assert_eq!(
            replace_section("## A\nold\n\n## B", "A", "").unwrap(),
            "## A\n\n## B"
        );
        assert_eq!(replace_section("## A\nold", "A", "").unwrap(), "## A\n");
        assert!(replace_section("## A", "Z", "x").is_err());
    }

    #[test]
    fn update_frontmatter_changes_adds_and_removes_properties() {
        let note = "---\ntitle: Old\ntags:\n  - a\n  - b\n# comment\nstatus: draft\n---\nBody";
        let properties = json!({"title": "New", "tags": ["x"], "status": null, "rating": 5});
        let out = update_frontmatter(note, properties.as_object().unwrap());
        assert_eq!(
            out,
            "---\ntitle: New\ntags:\n  - x\n# comment\nrating: 5\n---\nBody"
        );
    }

    #[test]
    fn update_frontmatter_replaces_unindented_list_items_with_their_property() {
        // YAML also allows `- item` lines without indentation, a bare `-` and blank lines.
        let note = "---\ntags:\n- a\n-\n\ntitle: T\n---\nBody";
        let out = update_frontmatter(note, json!({"tags": ["x"]}).as_object().unwrap());
        assert_eq!(out, "---\ntags:\n  - x\ntitle: T\n---\nBody");
    }

    #[test]
    fn update_frontmatter_never_treats_a_comment_as_a_property() {
        let note = "---\n# note: keep me\ntitle: T\n---\nBody";
        let out = update_frontmatter(note, json!({"# note": null}).as_object().unwrap());
        assert_eq!(out, note);
    }

    #[test]
    fn update_frontmatter_creates_and_removes_the_block() {
        let added = update_frontmatter("Body", json!({"done": true}).as_object().unwrap());
        assert_eq!(added, "---\ndone: true\n---\nBody");
        let removed = update_frontmatter(&added, json!({"done": null}).as_object().unwrap());
        assert_eq!(removed, "Body");
        let unchanged = update_frontmatter("Body", json!({"missing": null}).as_object().unwrap());
        assert_eq!(unchanged, "Body");
    }

    #[test]
    fn update_frontmatter_keeps_unparsed_lines_and_crlf_frontmatter() {
        let note = "---\r\n\"quoted key\": 1\r\nloose line\r\n---\r\nBody";
        let out = update_frontmatter(note, json!({"quoted key": 2}).as_object().unwrap());
        assert_eq!(out, "---\nquoted key: 2\nloose line\n---\nBody");
    }

    #[test]
    fn rendered_values_parse_back_to_the_same_values() {
        let properties = json!({
            "plain": "hello world",
            "number_like": "42",
            "bool_like": "true",
            "colon": "a: b",
            "hash": "x #y",
            "list_like": "[a]",
            "padded": " x ",
            "multiline": "a\nb",
            "empty": "",
            "float": 1.5,
            "empty_list": [],
            "list": ["#tag", "7", 7, "plain"],
            "key: odd": "v",
        });
        let out = update_frontmatter("", properties.as_object().unwrap());
        let parsed = parse_frontmatter(&out);
        for key in [
            "plain",
            "number_like",
            "bool_like",
            "colon",
            "hash",
            "list_like",
            "padded",
            "float",
            "empty_list",
            "list",
        ] {
            assert_eq!(parsed[key], properties[key], "{key} in\n{out}");
        }
        assert!(out.contains("multiline: \"a\\nb\""), "{out}");
        assert!(out.contains("empty: \"\""), "{out}");
        assert!(out.contains("\"key: odd\": v"), "{out}");
    }
}
