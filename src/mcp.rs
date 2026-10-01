//! Model Context Protocol server: newline-delimited JSON-RPC 2.0 over stdio.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::tools::{SchemaFormat, ToolError, Toolbox};

/// Protocol revisions this server can speak, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

pub struct Server {
    toolbox: Toolbox,
}

impl Server {
    pub fn new(toolbox: Toolbox) -> Self {
        Self { toolbox }
    }

    /// Handles one raw line from the transport. Returns the serialized response, if any.
    pub fn handle_message(&self, line: &str) -> Option<String> {
        let response = match serde_json::from_str::<Value>(line) {
            Err(e) => Some(error_response(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            )),
            Ok(Value::Array(batch)) => {
                let responses: Vec<Value> = batch
                    .iter()
                    .filter_map(|r| self.handle_request(r))
                    .collect();
                (!responses.is_empty()).then_some(Value::Array(responses))
            }
            Ok(request) => self.handle_request(&request),
        };
        response.map(|r| r.to_string())
    }

    /// Handles one parsed JSON-RPC message. Notifications produce no response.
    pub fn handle_request(&self, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned();
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            // Responses from the client (no method, but a result/error) need no reply.
            if request.get("result").is_some() || request.get("error").is_some() {
                return None;
            }
            return Some(error_response(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "invalid request: missing method",
            ));
        };
        let id = id?; // notifications never get a response
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        Some(match self.dispatch(method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": self.toolbox.export(SchemaFormat::Mcp)})),
            "tools/call" => self.call_tool(params),
            _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
        }
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = requested
            .filter(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(v))
            .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "obsidian-connector", "version": env!("CARGO_PKG_VERSION")},
            "instructions": format!(
                "Tools for the Obsidian vault at {}. Notes are markdown files addressed by vault-relative paths \
                 such as 'Projects/Plan' (the .md extension is optional). Use search_notes or list_notes to find \
                 notes before reading them, and [[wikilinks]] to link notes together.",
                self.toolbox.vault().root().display()
            ),
        })
    }

    fn call_tool(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params.get("name").and_then(Value::as_str).ok_or((
            INVALID_PARAMS,
            "tools/call requires a tool name".to_string(),
        ))?;
        let args = params.get("arguments").cloned().unwrap_or(Value::Null);
        let (text, is_error) = match self.toolbox.call(name, &args) {
            Ok(text) => (text, false),
            Err(ToolError::UnknownTool(name)) => {
                return Err((INVALID_PARAMS, format!("unknown tool: {name}")));
            }
            Err(e) => (e.to_string(), true),
        };
        Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
    }

    /// Serves requests from `input` until EOF, writing one response per line to `output`.
    pub fn run(&self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_message(&line) {
                writeln!(output, "{response}")?;
                output.flush()?;
            }
        }
        Ok(())
    }
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use std::fs;
    use tempfile::TempDir;

    fn server(read_only: bool) -> (TempDir, Server) {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Hello.md"), "Hello world").unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, Server::new(Toolbox::new(vault, read_only)))
    }

    fn request(server: &Server, msg: Value) -> Value {
        server.handle_request(&msg).expect("expected a response")
    }

    #[test]
    fn initialize_echoes_supported_protocol_version() {
        let (_dir, s) = server(false);
        let resp = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
        );
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(resp["result"]["serverInfo"]["name"], "obsidian-connector");
        assert!(resp["result"]["capabilities"]["tools"].is_object());
        assert!(
            resp["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("Obsidian")
        );
    }

    #[test]
    fn initialize_falls_back_to_latest_version() {
        let (_dir, s) = server(false);
        let resp = request(
            &s,
            json!({"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}),
        );
        assert_eq!(resp["id"], "a");
        assert_eq!(
            resp["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
    }

    #[test]
    fn notifications_get_no_response() {
        let (_dir, s) = server(false);
        assert_eq!(
            s.handle_request(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
            None
        );
        assert_eq!(
            s.handle_request(&json!({"jsonrpc": "2.0", "method": "unknown/notification"})),
            None
        );
    }

    #[test]
    fn ping_returns_empty_result() {
        let (_dir, s) = server(false);
        assert_eq!(
            request(&s, json!({"jsonrpc": "2.0", "id": 7, "method": "ping"})),
            json!({"jsonrpc": "2.0", "id": 7, "result": {}})
        );
    }

    #[test]
    fn tools_list_returns_mcp_tool_definitions() {
        let (_dir, s) = server(true);
        let resp = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        );
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "read_note"));
        assert!(!tools.iter().any(|t| t["name"] == "delete_note"));
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
    }

    #[test]
    fn tools_call_returns_text_content() {
        let (_dir, s) = server(false);
        let resp = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "read_note", "arguments": {"path": "Hello"}}}),
        );
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(
            resp["result"]["content"],
            json!([{"type": "text", "text": "Hello world"}])
        );
    }

    #[test]
    fn tools_call_reports_tool_failures_as_error_results() {
        let (_dir, s) = server(false);
        let resp = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {"name": "read_note", "arguments": {"path": "Missing"}}}),
        );
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("not found")
        );
    }

    #[test]
    fn tools_call_with_unknown_tool_or_no_name_is_invalid_params() {
        let (_dir, s) = server(false);
        let unknown = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "nope"}}),
        );
        assert_eq!(unknown["error"]["code"], INVALID_PARAMS);
        let nameless = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {}}),
        );
        assert_eq!(nameless["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let (_dir, s) = server(false);
        let resp = request(
            &s,
            json!({"jsonrpc": "2.0", "id": 8, "method": "resources/teleport"}),
        );
        assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(resp["id"], 8);
    }

    #[test]
    fn malformed_requests_are_invalid_request() {
        let (_dir, s) = server(false);
        let resp = request(&s, json!({"jsonrpc": "2.0", "id": 9}));
        assert_eq!(resp["error"]["code"], INVALID_REQUEST);
        let resp = request(&s, json!("just a string"));
        assert_eq!(resp["error"]["code"], INVALID_REQUEST);
        assert_eq!(resp["id"], Value::Null);
    }

    #[test]
    fn handle_message_reports_parse_errors() {
        let (_dir, s) = server(false);
        let resp: Value = serde_json::from_str(&s.handle_message("{not json").unwrap()).unwrap();
        assert_eq!(resp["error"]["code"], PARSE_ERROR);
        assert_eq!(resp["id"], Value::Null);
    }

    #[test]
    fn handle_message_supports_batches() {
        let (_dir, s) = server(false);
        let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"ping"}]"#;
        let resp: Value = serde_json::from_str(&s.handle_message(batch).unwrap()).unwrap();
        let ids: Vec<_> = resp
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].clone())
            .collect();
        assert_eq!(ids, vec![json!(1), json!(2)]);
        assert_eq!(
            s.handle_message(r#"[{"jsonrpc":"2.0","method":"notifications/initialized"}]"#),
            None
        );
    }

    #[test]
    fn run_writes_one_line_per_response_and_skips_blank_lines() {
        let (_dir, s) = server(false);
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        s.run(input.as_bytes(), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["id"], 2);
    }
}
