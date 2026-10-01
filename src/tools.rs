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
pub struct ToolDefinition {
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
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        definitions(self.read_only)
    }

    /// Tool definitions rendered in the requested schema dialect.
    pub fn export(&self, format: SchemaFormat) -> Value {
        export(format, self.read_only)
    }

    /// Runs a tool and returns its textual result.
    pub fn call(&self, name: &str, arguments: &Value) -> Result<String, ToolError> {
        let definition = all_definitions()
            .into_iter()
            .find(|definition| definition.name == name)
            .ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
        if definition.writes && self.read_only {
            return Err(ToolError::ReadOnly(name.to_string()));
        }
        let arguments = Arguments::new(arguments)?;
        let vault = &self.vault;
        match name {
            "list_notes" => to_json(&vault.list_notes(arguments.optional_string("folder")?)?),
            "read_note" => Ok(vault.read_note(arguments.required_string("path")?)?),
            "create_note" => {
                let overwrite = arguments.optional_bool("overwrite")?.unwrap_or(false);
                let path = vault.write_note(
                    arguments.required_string("path")?,
                    arguments.required_string("content")?,
                    overwrite,
                )?;
                Ok(format!("Saved {path}"))
            }
            "append_to_note" => Ok(format!(
                "Appended to {}",
                vault.append_note(
                    arguments.required_string("path")?,
                    arguments.required_string("content")?
                )?
            )),
            "delete_note" => Ok(format!(
                "Deleted {}",
                vault.delete_note(arguments.required_string("path")?)?
            )),
            "move_note" => Ok(format!(
                "Moved to {}",
                vault.move_note(
                    arguments.required_string("from")?,
                    arguments.required_string("to")?
                )?
            )),
            "search_notes" => {
                let limit = arguments
                    .optional_count("limit")?
                    .unwrap_or(DEFAULT_SEARCH_LIMIT);
                to_json(&vault.search(arguments.required_string("query")?, limit)?)
            }
            "get_backlinks" => to_json(&vault.backlinks(arguments.required_string("path")?)?),
            "list_tags" => to_json(&vault.tags()?),
            "find_notes_by_tag" => {
                to_json(&vault.notes_with_tag(arguments.required_string("tag")?)?)
            }
            "get_note_info" => to_json(&vault.note_info(arguments.required_string("path")?)?),
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

/// The tools available with or without write access.
pub fn definitions(read_only: bool) -> Vec<ToolDefinition> {
    all_definitions()
        .into_iter()
        .filter(|definition| !(read_only && definition.writes))
        .collect()
}

/// Tool definitions rendered in the requested schema dialect.
pub fn export(format: SchemaFormat, read_only: bool) -> Value {
    let tools = definitions(read_only).into_iter().map(|definition| match format {
        SchemaFormat::Mcp => json!({"name": definition.name, "description": definition.description, "inputSchema": definition.input_schema}),
        SchemaFormat::Anthropic => json!({"name": definition.name, "description": definition.description, "input_schema": definition.input_schema}),
        SchemaFormat::OpenAi => json!({
            "type": "function",
            "function": {"name": definition.name, "description": definition.description, "parameters": definition.input_schema},
        }),
    });
    Value::Array(tools.collect())
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|error| ToolError::InvalidArguments(error.to_string()))
}

/// Typed accessors over a JSON arguments object.
struct Arguments<'a> {
    fields: Option<&'a serde_json::Map<String, Value>>,
}

impl<'a> Arguments<'a> {
    fn new(arguments: &'a Value) -> Result<Self, ToolError> {
        match arguments {
            Value::Null => Ok(Self { fields: None }),
            Value::Object(map) => Ok(Self { fields: Some(map) }),
            _ => Err(ToolError::InvalidArguments(
                "arguments must be a JSON object".into(),
            )),
        }
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.fields
            .and_then(|fields| fields.get(key))
            .filter(|value| !value.is_null())
    }

    fn required_string(&self, key: &str) -> Result<&'a str, ToolError> {
        self.optional_string(key)?
            .ok_or_else(|| ToolError::InvalidArguments(format!("missing required string '{key}'")))
    }

    fn optional_string(&self, key: &str) -> Result<Option<&'a str>, ToolError> {
        self.get(key)
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidArguments(format!("'{key}' must be a string")))
            })
            .transpose()
    }

    fn optional_bool(&self, key: &str) -> Result<Option<bool>, ToolError> {
        self.get(key)
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    ToolError::InvalidArguments(format!("'{key}' must be a boolean"))
                })
            })
            .transpose()
    }

    fn optional_count(&self, key: &str) -> Result<Option<usize>, ToolError> {
        self.get(key)
            .map(|value| {
                value.as_u64().map(|count| count as usize).ok_or_else(|| {
                    ToolError::InvalidArguments(format!("'{key}' must be a non-negative integer"))
                })
            })
            .transpose()
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn note_path_property() -> Value {
    json!({"type": "string", "description": "Vault-relative note path, e.g. 'Projects/Plan' (the .md extension is optional)"})
}

fn all_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_notes",
            description: "List the markdown notes in the vault, optionally only inside one folder.",
            input_schema: schema(
                json!({"folder": {"type": "string", "description": "Vault-relative folder to list; omit for the whole vault"}}),
                &[],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "read_note",
            description: "Read the full markdown content of a note, including its frontmatter.",
            input_schema: schema(json!({"path": note_path_property()}), &["path"]),
            writes: false,
        },
        ToolDefinition {
            name: "create_note",
            description: "Create a note (and any missing folders). Fails if the note exists unless overwrite is true.",
            input_schema: schema(
                json!({
                    "path": note_path_property(),
                    "content": {"type": "string", "description": "Markdown content of the note"},
                    "overwrite": {"type": "boolean", "description": "Replace the note if it already exists (default false)"},
                }),
                &["path", "content"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "append_to_note",
            description: "Append markdown to the end of a note on a new line, creating the note if it does not exist.",
            input_schema: schema(
                json!({"path": note_path_property(), "content": {"type": "string", "description": "Markdown to append"}}),
                &["path", "content"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "delete_note",
            description: "Permanently delete a note.",
            input_schema: schema(json!({"path": note_path_property()}), &["path"]),
            writes: true,
        },
        ToolDefinition {
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
        ToolDefinition {
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
        ToolDefinition {
            name: "get_backlinks",
            description: "List the notes that link to the given note with [[wikilinks]] or ![[embeds]].",
            input_schema: schema(json!({"path": note_path_property()}), &["path"]),
            writes: false,
        },
        ToolDefinition {
            name: "list_tags",
            description: "List every tag used in the vault with the number of notes that use it.",
            input_schema: schema(json!({}), &[]),
            writes: false,
        },
        ToolDefinition {
            name: "find_notes_by_tag",
            description: "List the notes carrying a tag (frontmatter or inline). Nested tags match their parent, e.g. 'project' matches 'project/alpha'.",
            input_schema: schema(
                json!({"tag": {"type": "string", "description": "Tag name, with or without the leading #"}}),
                &["tag"],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "get_note_info",
            description: "Get a note's metadata: frontmatter properties, tags, outgoing links, backlinks and size.",
            input_schema: schema(json!({"path": note_path_property()}), &["path"]),
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

    fn toolbox_with_sample_notes(read_only: bool) -> (TempDir, Toolbox) {
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
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let definitions = toolbox.definitions();
        let names: HashSet<_> = definitions
            .iter()
            .map(|definition| definition.name)
            .collect();
        assert_eq!(names.len(), definitions.len());
        for definition in &definitions {
            assert!(
                !definition.description.is_empty(),
                "{} has no description",
                definition.name
            );
            assert_eq!(
                definition.input_schema["type"], "object",
                "{}",
                definition.name
            );
            let properties = definition.input_schema["properties"].as_object().unwrap();
            for required in definition.input_schema["required"].as_array().unwrap() {
                assert!(
                    properties.contains_key(required.as_str().unwrap()),
                    "{} requires unknown {required}",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn definitions_cover_core_operations() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let names: Vec<_> = toolbox
            .definitions()
            .iter()
            .map(|definition| definition.name)
            .collect();
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
        let (_dir, toolbox) = toolbox_with_sample_notes(true);
        let definitions = toolbox.definitions();
        assert!(definitions.iter().all(|definition| !definition.writes));
        assert!(
            definitions
                .iter()
                .any(|definition| definition.name == "read_note")
        );
        assert!(
            !definitions
                .iter()
                .any(|definition| definition.name == "create_note")
        );
    }

    #[test]
    fn read_only_mode_refuses_write_calls() {
        let (dir, toolbox) = toolbox_with_sample_notes(true);
        let error = toolbox
            .call("create_note", &json!({"path": "X", "content": "y"}))
            .unwrap_err();
        assert!(matches!(error, ToolError::ReadOnly(_)));
        assert!(!dir.path().join("X.md").exists());
    }

    #[test]
    fn export_mcp_format() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let exported = toolbox.export(SchemaFormat::Mcp);
        let first = &exported.as_array().unwrap()[0];
        assert!(first["name"].is_string());
        assert!(first["description"].is_string());
        assert_eq!(first["inputSchema"]["type"], "object");
    }

    #[test]
    fn export_openai_format() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let exported = toolbox.export(SchemaFormat::OpenAi);
        let first = &exported.as_array().unwrap()[0];
        assert_eq!(first["type"], "function");
        assert!(first["function"]["name"].is_string());
        assert_eq!(first["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn export_anthropic_format() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let exported = toolbox.export(SchemaFormat::Anthropic);
        let first = &exported.as_array().unwrap()[0];
        assert!(first["name"].is_string());
        assert_eq!(first["input_schema"]["type"], "object");
    }

    #[test]
    fn unknown_tool_is_an_error() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        assert!(matches!(
            toolbox.call("nope", &json!({})),
            Err(ToolError::UnknownTool(_))
        ));
    }

    #[test]
    fn missing_or_mistyped_arguments_are_reported() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        assert!(matches!(
            toolbox.call("read_note", &json!({})),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            toolbox.call("read_note", &json!({"path": 5})),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            toolbox.call("read_note", &json!(null)),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            toolbox.call("search_notes", &json!({"query": "x", "limit": "ten"})),
            Err(ToolError::InvalidArguments(_))
        ));
    }

    #[test]
    fn list_notes_returns_json_array() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let output: Value =
            serde_json::from_str(&toolbox.call("list_notes", &json!({})).unwrap()).unwrap();
        assert_eq!(output, json!(["Hello.md", "World.md"]));
    }

    #[test]
    fn read_note_returns_raw_content() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        assert!(
            toolbox
                .call("read_note", &json!({"path": "World"}))
                .unwrap()
                .starts_with("The world")
        );
    }

    #[test]
    fn create_append_move_delete_roundtrip() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        toolbox
            .call(
                "create_note",
                &json!({"path": "Inbox/Idea", "content": "one"}),
            )
            .unwrap();
        assert!(
            toolbox
                .call(
                    "create_note",
                    &json!({"path": "Inbox/Idea", "content": "x"})
                )
                .is_err()
        );
        toolbox
            .call(
                "create_note",
                &json!({"path": "Inbox/Idea", "content": "two", "overwrite": true}),
            )
            .unwrap();
        toolbox
            .call(
                "append_to_note",
                &json!({"path": "Inbox/Idea", "content": "three"}),
            )
            .unwrap();
        assert_eq!(
            toolbox
                .call("read_note", &json!({"path": "Inbox/Idea"}))
                .unwrap(),
            "two\nthree"
        );
        let moved = toolbox
            .call(
                "move_note",
                &json!({"from": "Inbox/Idea", "to": "Done/Idea"}),
            )
            .unwrap();
        assert!(moved.contains("Done/Idea.md"));
        let deleted = toolbox
            .call("delete_note", &json!({"path": "Done/Idea"}))
            .unwrap();
        assert!(deleted.contains("Done/Idea.md"));
        assert!(
            toolbox
                .call("read_note", &json!({"path": "Done/Idea"}))
                .is_err()
        );
    }

    #[test]
    fn search_notes_returns_structured_results() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let output: Value = serde_json::from_str(
            &toolbox
                .call("search_notes", &json!({"query": "world"}))
                .unwrap(),
        )
        .unwrap();
        let paths: Vec<_> = output
            .as_array()
            .unwrap()
            .iter()
            .map(|result| result["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["Hello.md", "World.md"]);
        let limited: Value = serde_json::from_str(
            &toolbox
                .call("search_notes", &json!({"query": "world", "limit": 1}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(limited.as_array().unwrap().len(), 1);
    }

    #[test]
    fn graph_and_tag_tools() {
        let (_dir, toolbox) = toolbox_with_sample_notes(false);
        let backlinks: Value = serde_json::from_str(
            &toolbox
                .call("get_backlinks", &json!({"path": "World"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(backlinks, json!(["Hello.md"]));
        let tags: Value =
            serde_json::from_str(&toolbox.call("list_tags", &json!({})).unwrap()).unwrap();
        assert_eq!(tags, json!({"greeting": 1, "place": 1}));
        let tagged: Value = serde_json::from_str(
            &toolbox
                .call("find_notes_by_tag", &json!({"tag": "#place"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(tagged, json!(["World.md"]));
        let info: Value = serde_json::from_str(
            &toolbox
                .call("get_note_info", &json!({"path": "Hello"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(info["links"], json!(["World"]));
        assert_eq!(info["tags"], json!(["greeting"]));
    }

    #[test]
    fn every_property_has_a_type_and_description() {
        for definition in definitions(false) {
            let properties = definition.input_schema["properties"].as_object().unwrap();
            for (property_name, property) in properties {
                assert!(
                    property["type"].is_string(),
                    "{}.{property_name} has no type",
                    definition.name
                );
                assert!(
                    property["description"].is_string(),
                    "{}.{property_name} has no description",
                    definition.name
                );
            }
        }
    }
}
