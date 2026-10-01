//! Pure parsing helpers for Obsidian-flavoured markdown: frontmatter, tags and wikilinks.

use serde_json::{Map, Value};

/// Splits a note into its YAML frontmatter (without the `---` fences) and the body.
pub fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let Some(rest) = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))
    else {
        return (None, content);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let yaml = rest[..offset].trim_end_matches(['\r', '\n']);
            return (Some(yaml), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, content)
}

/// Parses the subset of YAML that Obsidian properties use: scalars, inline lists and block lists.
pub fn parse_frontmatter(content: &str) -> Map<String, Value> {
    let mut map = Map::new();
    let Some(yaml) = split_frontmatter(content).0 else {
        return map;
    };
    let mut current_key: Option<String> = None;
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) =
            trimmed
                .strip_prefix("- ")
                .or(if trimmed == "-" { Some("") } else { None })
        {
            if let Some(key) = &current_key {
                let entry = map.entry(key.clone()).or_insert(Value::Null);
                if !entry.is_array() {
                    *entry = Value::Array(Vec::new());
                }
                if let Value::Array(items) = entry {
                    items.push(parse_scalar(item));
                }
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            continue; // nested structures are not supported
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = unquote(key.trim()).to_string();
            map.insert(key.clone(), parse_value(value.trim()));
            current_key = Some(key);
        }
    }
    map
}

fn parse_value(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return Value::Array(
            inner
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(parse_scalar)
                .collect(),
        );
    }
    parse_scalar(raw)
}

fn parse_scalar(raw: &str) -> Value {
    let raw = raw.trim();
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        return Value::String(raw[1..raw.len() - 1].to_string());
    }
    match raw {
        "" | "~" | "null" => return Value::Null,
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Value::from(n);
    }
    if let Ok(f) = raw.parse::<f64>()
        && f.is_finite()
    {
        return Value::from(f);
    }
    Value::String(raw.to_string())
}

fn unquote(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'')
}

/// Removes fenced code blocks and inline code spans so they are not scanned for tags or links.
fn strip_code(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push('\n');
            continue;
        }
        for (i, segment) in line.split('`').enumerate() {
            if i % 2 == 0 {
                out.push_str(segment);
            } else {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !item.is_empty() && !list.iter().any(|existing| existing == item) {
        list.push(item.to_string());
    }
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// Returns the unique tags of a note (without `#`), from frontmatter and inline `#tags`, in order of appearance.
pub fn extract_tags(content: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let frontmatter = parse_frontmatter(content);
    for key in ["tags", "tag"] {
        match frontmatter.get(key) {
            Some(Value::Array(items)) => {
                for item in items {
                    if let Some(s) = item.as_str() {
                        push_unique(&mut tags, s.trim_start_matches('#'));
                    }
                }
            }
            Some(Value::String(s)) => {
                for part in s.split(|c: char| c == ',' || c.is_whitespace()) {
                    push_unique(&mut tags, part.trim_start_matches('#'));
                }
            }
            _ => {}
        }
    }

    let body = strip_code(split_frontmatter(content).1);
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let preceded_ok = i == 0 || chars[i - 1].is_whitespace();
        if chars[i] == '#' && preceded_ok {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && is_tag_char(chars[end]) {
                end += 1;
            }
            let tag: String = chars[start..end].iter().collect();
            let tag = tag.trim_end_matches('/');
            if tag.chars().any(|c| !c.is_ascii_digit()) {
                push_unique(&mut tags, tag);
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    tags
}

/// Returns the unique link targets of `[[wikilinks]]` and `![[embeds]]`, without aliases or headings.
pub fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let body = strip_code(split_frontmatter(content).1);
    let mut rest = body.as_str();
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let target = inner
            .split(['|', '#', '^'])
            .next()
            .unwrap_or_default()
            .trim();
        push_unique(&mut links, target);
        rest = &after[end + 2..];
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn split_frontmatter_returns_none_without_fences() {
        assert_eq!(split_frontmatter("# Title\nbody"), (None, "# Title\nbody"));
    }

    #[test]
    fn split_frontmatter_separates_yaml_and_body() {
        let note = "---\ntitle: Hi\n---\n# Body\n";
        assert_eq!(split_frontmatter(note), (Some("title: Hi"), "# Body\n"));
    }

    #[test]
    fn split_frontmatter_handles_crlf() {
        let note = "---\r\ntitle: Hi\r\n---\r\nbody";
        assert_eq!(split_frontmatter(note), (Some("title: Hi"), "body"));
    }

    #[test]
    fn split_frontmatter_ignores_unterminated_block() {
        let note = "---\ntitle: Hi\nbody";
        assert_eq!(split_frontmatter(note), (None, note));
    }

    #[test]
    fn split_frontmatter_handles_empty_block() {
        assert_eq!(split_frontmatter("---\n---\nbody"), (Some(""), "body"));
    }

    #[test]
    fn parse_frontmatter_reads_scalars() {
        let fm =
            parse_frontmatter("---\ntitle: My Note\ncount: 3\ndone: true\nquoted: \"a: b\"\n---\n");
        assert_eq!(fm["title"], json!("My Note"));
        assert_eq!(fm["count"], json!(3));
        assert_eq!(fm["done"], json!(true));
        assert_eq!(fm["quoted"], json!("a: b"));
    }

    #[test]
    fn parse_frontmatter_reads_inline_and_block_lists() {
        let fm = parse_frontmatter("---\ntags: [a, \"b\"]\naliases:\n  - One\n  - Two\n---\n");
        assert_eq!(fm["tags"], json!(["a", "b"]));
        assert_eq!(fm["aliases"], json!(["One", "Two"]));
    }

    #[test]
    fn parse_frontmatter_treats_empty_value_as_null() {
        let fm = parse_frontmatter("---\nempty:\n---\n");
        assert_eq!(fm["empty"], Value::Null);
    }

    #[test]
    fn parse_frontmatter_is_empty_without_frontmatter() {
        assert!(parse_frontmatter("no frontmatter").is_empty());
    }

    #[test]
    fn extract_tags_finds_inline_tags() {
        assert_eq!(
            extract_tags("Some #idea and #project/alpha here."),
            vec!["idea", "project/alpha"]
        );
    }

    #[test]
    fn extract_tags_skips_headings_numbers_and_urls() {
        let note = "# Heading\n## Sub\nIssue #123 at http://x.com/#anchor and a#b";
        assert!(extract_tags(note).is_empty());
    }

    #[test]
    fn extract_tags_skips_code() {
        let note = "```\n#notatag\n```\nuse `#nope` but #yes";
        assert_eq!(extract_tags(note), vec!["yes"]);
    }

    #[test]
    fn extract_tags_includes_frontmatter_tags_and_dedupes() {
        let note = "---\ntags: [alpha, \"#beta\"]\ntag: gamma\n---\n#alpha #delta";
        assert_eq!(extract_tags(note), vec!["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    fn extract_tags_accepts_space_separated_frontmatter_string() {
        let note = "---\ntags: one two\n---\n";
        assert_eq!(extract_tags(note), vec!["one", "two"]);
    }

    #[test]
    fn extract_wikilinks_strips_alias_heading_and_block() {
        let note = "See [[Note A]], [[Folder/Note B|alias]], [[Note C#Heading]], [[Note D^block]] and ![[image.png]].";
        assert_eq!(
            extract_wikilinks(note),
            vec!["Note A", "Folder/Note B", "Note C", "Note D", "image.png"]
        );
    }

    #[test]
    fn extract_wikilinks_dedupes_and_skips_empty_and_code() {
        let note = "[[A]] [[A|again]] [[]] [[#Local heading]] `[[Code]]`";
        assert_eq!(extract_wikilinks(note), vec!["A"]);
    }
}
