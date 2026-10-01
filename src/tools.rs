//! Agent-facing tools: one definition per vault operation, shared by the MCP server and the CLI.

use serde_json::{Value, json};

use crate::vault::{Vault, VaultError};

pub const DEFAULT_SEARCH_LIMIT: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool '{0}' modifies the vault, but the connector is running in read-only mode")]
    ReadOnly(String),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

/// Output schema dialects understood by different agent frameworks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SchemaFormat {
    Mcp,
    #[value(name = "openai")]
    OpenAi,
    Anthropic,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub writes: bool,
}

pub struct Toolbox {
    vault: Vault,
    read_only: bool,
}

impl Toolbox {
    pub fn new(vault: Vault, read_only: bool) -> Self {
        Self { vault, read_only }
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// The tools available in the current mode.
    pub fn definitions(&self) -> Vec<ToolDef> {
        definitions(self.read_only)
    }

    /// Tool definitions rendered in the requested schema dialect.
    pub fn export(&self, format: SchemaFormat) -> Value {
        export(format, self.read_only)
    }

    /// Runs a tool and returns its textual result.
    pub fn call(&self, name: &str, args: &Value) -> Result<String, ToolError> {
        let def = all_definitions()
            .into_iter()
            .find(|d| d.name == name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        if def.writes && self.read_only {
            return Err(ToolError::ReadOnly(name.to_string()));
        }
        let args = Args::new(args)?;
        let v = &self.vault;
        match name {
            "list_notes" => to_json(&v.list_notes(args.opt_str("folder")?)?),
            "read_note" => Ok(v.read_note(args.str("path")?)?),
            "create_note" => {
                let overwrite = args.opt_bool("overwrite")?.unwrap_or(false);
                let path = v.write_note(args.str("path")?, args.str("content")?, overwrite)?;
                Ok(format!("Saved {path}"))
            }
            "append_to_note" => Ok(format!(
                "Appended to {}",
                v.append_note(args.str("path")?, args.str("content")?)?
            )),
            "delete_note" => Ok(format!("Deleted {}", v.delete_note(args.str("path")?)?)),
            "move_note" => Ok(format!(
                "Moved to {}",
                v.move_note(args.str("from")?, args.str("to")?)?
            )),
            "search_notes" => {
                let limit = args.opt_usize("limit")?.unwrap_or(DEFAULT_SEARCH_LIMIT);
                to_json(&v.search(args.str("query")?, limit)?)
            }
            "get_backlinks" => to_json(&v.backlinks(args.str("path")?)?),
            "list_tags" => to_json(&v.tags()?),
            "find_notes_by_tag" => to_json(&v.notes_with_tag(args.str("tag")?)?),
            "get_note_info" => to_json(&v.note_info(args.str("path")?)?),
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

/// The tools available with or without write access.
pub fn definitions(read_only: bool) -> Vec<ToolDef> {
    all_definitions()
        .into_iter()
        .filter(|d| !(read_only && d.writes))
        .collect()
}

/// Tool definitions rendered in the requested schema dialect.
pub fn export(format: SchemaFormat, read_only: bool) -> Value {
    let tools = definitions(read_only).into_iter().map(|d| match format {
        SchemaFormat::Mcp => json!({"name": d.name, "description": d.description, "inputSchema": d.input_schema}),
        SchemaFormat::Anthropic => json!({"name": d.name, "description": d.description, "input_schema": d.input_schema}),
        SchemaFormat::OpenAi => json!({
            "type": "function",
            "function": {"name": d.name, "description": d.description, "parameters": d.input_schema},
        }),
    });
    Value::Array(tools.collect())
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value).map_err(|e| ToolError::InvalidArguments(e.to_string()))
}

/// Typed accessors over a JSON arguments object.
struct Args<'a>(Option<&'a serde_json::Map<String, Value>>);

impl<'a> Args<'a> {
    fn new(args: &'a Value) -> Result<Self, ToolError> {
        match args {
            Value::Null => Ok(Self(None)),
            Value::Object(map) => Ok(Self(Some(map))),
            _ => Err(ToolError::InvalidArguments(
                "arguments must be a JSON object".into(),
            )),
        }
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.and_then(|m| m.get(key)).filter(|v| !v.is_null())
    }

    fn str(&self, key: &str) -> Result<&'a str, ToolError> {
        self.opt_str(key)?
            .ok_or_else(|| ToolError::InvalidArguments(format!("missing required string '{key}'")))
    }

    fn opt_str(&self, key: &str) -> Result<Option<&'a str>, ToolError> {
        self.get(key)
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| ToolError::InvalidArguments(format!("'{key}' must be a string")))
            })
            .transpose()
    }

    fn opt_bool(&self, key: &str) -> Result<Option<bool>, ToolError> {
        self.get(key)
            .map(|v| {
                v.as_bool().ok_or_else(|| {
                    ToolError::InvalidArguments(format!("'{key}' must be a boolean"))
                })
            })
            .transpose()
    }

    fn opt_usize(&self, key: &str) -> Result<Option<usize>, ToolError> {
        self.get(key)
            .map(|v| {
                v.as_u64().map(|n| n as usize).ok_or_else(|| {
                    ToolError::InvalidArguments(format!("'{key}' must be a non-negative integer"))
                })
            })
            .transpose()
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn path_prop() -> Value {
    json!({"type": "string", "description": "Vault-relative note path, e.g. 'Projects/Plan' (the .md extension is optional)"})
}

fn all_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_notes",
            description: "List the markdown notes in the vault, optionally only inside one folder.",
            input_schema: schema(
                json!({"folder": {"type": "string", "description": "Vault-relative folder to list; omit for the whole vault"}}),
                &[],
            ),
            writes: false,
        },
        ToolDef {
            name: "read_note",
            description: "Read the full markdown content of a note, including its frontmatter.",
            input_schema: schema(json!({"path": path_prop()}), &["path"]),
            writes: false,
        },
        ToolDef {
            name: "create_note",
            description: "Create a note (and any missing folders). Fails if the note exists unless overwrite is true.",
            input_schema: schema(
                json!({
                    "path": path_prop(),
                    "content": {"type": "string", "description": "Markdown content of the note"},
                    "overwrite": {"type": "boolean", "description": "Replace the note if it already exists (default false)"},
                }),
                &["path", "content"],
            ),
            writes: true,
        },
        ToolDef {
            name: "append_to_note",
            description: "Append markdown to the end of a note on a new line, creating the note if it does not exist.",
            input_schema: schema(
                json!({"path": path_prop(), "content": {"type": "string", "description": "Markdown to append"}}),
                &["path", "content"],
            ),
            writes: true,
        },
        ToolDef {
            name: "delete_note",
            description: "Permanently delete a note.",
            input_schema: schema(json!({"path": path_prop()}), &["path"]),
            writes: true,
        },
        ToolDef {
            name: "move_note",
            description: "Move or rename a note. Fails if the destination already exists. Links to it are not rewritten.",
            input_schema: schema(
                json!({
                    "from": {"type": "string", "description": "Current vault-relative note path"},
                    "to": {"type": "string", "description": "New vault-relative note path"},
                }),
                &["from", "to"],
            ),
            writes: true,
        },
        ToolDef {
            name: "search_notes",
            description: "Case-insensitive full-text search. Returns notes containing every word of the query (in the content or file path) with the matching lines.",
            input_schema: schema(
                json!({
                    "query": {"type": "string", "description": "Words to search for"},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum number of notes to return (default 20)"},
                }),
                &["query"],
            ),
            writes: false,
        },
        ToolDef {
            name: "get_backlinks",
            description: "List the notes that link to the given note with [[wikilinks]] or ![[embeds]].",
            input_schema: schema(json!({"path": path_prop()}), &["path"]),
            writes: false,
        },
        ToolDef {
            name: "list_tags",
            description: "List every tag used in the vault with the number of notes that use it.",
            input_schema: schema(json!({}), &[]),
            writes: false,
        },
        ToolDef {
            name: "find_notes_by_tag",
            description: "List the notes carrying a tag (frontmatter or inline). Nested tags match their parent, e.g. 'project' matches 'project/alpha'.",
            input_schema: schema(
                json!({"tag": {"type": "string", "description": "Tag name, with or without the leading #"}}),
                &["tag"],
            ),
            writes: false,
        },
        ToolDef {
            name: "get_note_info",
            description: "Get a note's metadata: frontmatter properties, tags, outgoing links, backlinks and size.",
            input_schema: schema(json!({"path": path_prop()}), &["path"]),
            writes: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use tempfile::TempDir;

    fn toolbox(read_only: bool) -> (TempDir, Toolbox) {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Hello.md"),
            "---\ntags: [greeting]\n---\nHello [[World]]",
        )
        .unwrap();
        fs::write(dir.path().join("World.md"), "The world #place").unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, Toolbox::new(vault, read_only))
    }

    #[test]
    fn definitions_have_unique_names_and_valid_schemas() {
        let (_dir, tb) = toolbox(false);
        let defs = tb.definitions();
        let names: HashSet<_> = defs.iter().map(|d| d.name).collect();
        assert_eq!(names.len(), defs.len());
        for def in &defs {
            assert!(
                !def.description.is_empty(),
                "{} has no description",
                def.name
            );
            assert_eq!(def.input_schema["type"], "object", "{}", def.name);
            let props = def.input_schema["properties"].as_object().unwrap();
            for required in def.input_schema["required"].as_array().unwrap() {
                assert!(
                    props.contains_key(required.as_str().unwrap()),
                    "{} requires unknown {required}",
                    def.name
                );
            }
        }
    }

    #[test]
    fn definitions_cover_core_operations() {
        let (_dir, tb) = toolbox(false);
        let names: Vec<_> = tb.definitions().iter().map(|d| d.name).collect();
        for expected in [
            "list_notes",
            "read_note",
            "create_note",
            "append_to_note",
            "delete_note",
            "move_note",
            "search_notes",
            "get_backlinks",
            "list_tags",
            "find_notes_by_tag",
            "get_note_info",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn read_only_mode_hides_write_tools() {
        let (_dir, tb) = toolbox(true);
        let defs = tb.definitions();
        assert!(defs.iter().all(|d| !d.writes));
        assert!(defs.iter().any(|d| d.name == "read_note"));
        assert!(!defs.iter().any(|d| d.name == "create_note"));
    }

    #[test]
    fn read_only_mode_refuses_write_calls() {
        let (dir, tb) = toolbox(true);
        let err = tb
            .call("create_note", &json!({"path": "X", "content": "y"}))
            .unwrap_err();
        assert!(matches!(err, ToolError::ReadOnly(_)));
        assert!(!dir.path().join("X.md").exists());
    }

    #[test]
    fn export_mcp_format() {
        let (_dir, tb) = toolbox(false);
        let exported = tb.export(SchemaFormat::Mcp);
        let first = &exported.as_array().unwrap()[0];
        assert!(first["name"].is_string());
        assert!(first["description"].is_string());
        assert_eq!(first["inputSchema"]["type"], "object");
    }

    #[test]
    fn export_openai_format() {
        let (_dir, tb) = toolbox(false);
        let exported = tb.export(SchemaFormat::OpenAi);
        let first = &exported.as_array().unwrap()[0];
        assert_eq!(first["type"], "function");
        assert!(first["function"]["name"].is_string());
        assert_eq!(first["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn export_anthropic_format() {
        let (_dir, tb) = toolbox(false);
        let exported = tb.export(SchemaFormat::Anthropic);
        let first = &exported.as_array().unwrap()[0];
        assert!(first["name"].is_string());
        assert_eq!(first["input_schema"]["type"], "object");
    }

    #[test]
    fn unknown_tool_is_an_error() {
        let (_dir, tb) = toolbox(false);
        assert!(matches!(
            tb.call("nope", &json!({})),
            Err(ToolError::UnknownTool(_))
        ));
    }

    #[test]
    fn missing_or_mistyped_arguments_are_reported() {
        let (_dir, tb) = toolbox(false);
        assert!(matches!(
            tb.call("read_note", &json!({})),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            tb.call("read_note", &json!({"path": 5})),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            tb.call("read_note", &json!(null)),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            tb.call("search_notes", &json!({"query": "x", "limit": "ten"})),
            Err(ToolError::InvalidArguments(_))
        ));
    }

    #[test]
    fn list_notes_returns_json_array() {
        let (_dir, tb) = toolbox(false);
        let out: Value = serde_json::from_str(&tb.call("list_notes", &json!({})).unwrap()).unwrap();
        assert_eq!(out, json!(["Hello.md", "World.md"]));
    }

    #[test]
    fn read_note_returns_raw_content() {
        let (_dir, tb) = toolbox(false);
        assert!(
            tb.call("read_note", &json!({"path": "World"}))
                .unwrap()
                .starts_with("The world")
        );
    }

    #[test]
    fn create_append_move_delete_roundtrip() {
        let (_dir, tb) = toolbox(false);
        tb.call(
            "create_note",
            &json!({"path": "Inbox/Idea", "content": "one"}),
        )
        .unwrap();
        assert!(
            tb.call(
                "create_note",
                &json!({"path": "Inbox/Idea", "content": "x"})
            )
            .is_err()
        );
        tb.call(
            "create_note",
            &json!({"path": "Inbox/Idea", "content": "two", "overwrite": true}),
        )
        .unwrap();
        tb.call(
            "append_to_note",
            &json!({"path": "Inbox/Idea", "content": "three"}),
        )
        .unwrap();
        assert_eq!(
            tb.call("read_note", &json!({"path": "Inbox/Idea"}))
                .unwrap(),
            "two\nthree"
        );
        let moved = tb
            .call(
                "move_note",
                &json!({"from": "Inbox/Idea", "to": "Done/Idea"}),
            )
            .unwrap();
        assert!(moved.contains("Done/Idea.md"));
        let deleted = tb
            .call("delete_note", &json!({"path": "Done/Idea"}))
            .unwrap();
        assert!(deleted.contains("Done/Idea.md"));
        assert!(tb.call("read_note", &json!({"path": "Done/Idea"})).is_err());
    }

    #[test]
    fn search_notes_returns_structured_results() {
        let (_dir, tb) = toolbox(false);
        let out: Value =
            serde_json::from_str(&tb.call("search_notes", &json!({"query": "world"})).unwrap())
                .unwrap();
        let paths: Vec<_> = out
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["Hello.md", "World.md"]);
        let limited: Value = serde_json::from_str(
            &tb.call("search_notes", &json!({"query": "world", "limit": 1}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(limited.as_array().unwrap().len(), 1);
    }

    #[test]
    fn graph_and_tag_tools() {
        let (_dir, tb) = toolbox(false);
        let backlinks: Value =
            serde_json::from_str(&tb.call("get_backlinks", &json!({"path": "World"})).unwrap())
                .unwrap();
        assert_eq!(backlinks, json!(["Hello.md"]));
        let tags: Value = serde_json::from_str(&tb.call("list_tags", &json!({})).unwrap()).unwrap();
        assert_eq!(tags, json!({"greeting": 1, "place": 1}));
        let tagged: Value = serde_json::from_str(
            &tb.call("find_notes_by_tag", &json!({"tag": "#place"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(tagged, json!(["World.md"]));
        let info: Value =
            serde_json::from_str(&tb.call("get_note_info", &json!({"path": "Hello"})).unwrap())
                .unwrap();
        assert_eq!(info["links"], json!(["World"]));
        assert_eq!(info["tags"], json!(["greeting"]));
    }
}
