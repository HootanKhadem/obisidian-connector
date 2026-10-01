//! End-to-end tests that drive the compiled binary the way agents and users do.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use obsidian_connector::discovery::{config_dir_for, obsidian_config_paths};
use serde_json::{Value, json};
use tempfile::TempDir;

fn vault() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("Hello.md"), "Hello [[World]] #greeting").unwrap();
    fs::write(dir.path().join("World.md"), "The world").unwrap();
    dir
}

/// A command isolated from the developer's real Obsidian and client settings.
fn cmd(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_obsidian-connector"));
    cmd.env_remove("OBSIDIAN_VAULT")
        .env_remove("APPDATA")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join(".config"));
    cmd
}

fn run(mut command: Command, stdin: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn prints_version() {
    let home = TempDir::new().unwrap();
    let out = cmd(home.path()).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn tools_exports_schemas_without_a_vault() {
    let home = TempDir::new().unwrap();
    let out = cmd(home.path())
        .args(["tools", "--format", "openai"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let tools: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(tools[0]["type"], "function");

    let out = cmd(home.path())
        .args(["tools", "--read-only"])
        .output()
        .unwrap();
    let tools: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["name"] != "delete_note")
    );
}

#[test]
fn call_runs_a_tool_and_prints_its_result() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let out = cmd(home.path())
        .args(["call", "read_note", r#"{"path": "Hello"}"#, "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim_end(), "Hello [[World]] #greeting");
}

#[test]
fn call_reads_arguments_from_stdin_and_vault_from_env() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let mut command = cmd(home.path());
    command
        .args(["call", "get_backlinks", "-"])
        .env("OBSIDIAN_VAULT", v.path());
    let out = run(command, r#"{"path": "World"}"#);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&out)).unwrap(),
        json!(["Hello.md"])
    );
}

#[test]
fn call_without_arguments_uses_an_empty_object() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let out = cmd(home.path())
        .args(["call", "list_notes", "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&out)).unwrap(),
        json!(["Hello.md", "World.md"])
    );
}

#[test]
fn call_failures_exit_non_zero_with_a_message() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let out = cmd(home.path())
        .args(["call", "read_note", r#"{"path": "Missing"}"#, "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("not found"));

    let out = cmd(home.path())
        .args(["call", "read_note", "{bad", "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("JSON"));
}

#[test]
fn read_only_blocks_writes() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let out = cmd(home.path())
        .args([
            "call",
            "delete_note",
            r#"{"path": "Hello"}"#,
            "--read-only",
            "--vault",
        ])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(v.path().join("Hello.md").exists());
}

#[test]
fn missing_vault_is_explained() {
    let home = TempDir::new().unwrap();
    let out = cmd(home.path())
        .args(["call", "list_notes"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--vault"), "{}", stderr(&out));
}

#[test]
fn serve_speaks_mcp_over_stdio() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let mut command = cmd(home.path());
    command.args(["serve", "--vault"]).arg(v.path());
    let session = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "create_note", "arguments": {"path": "Inbox/New", "content": "made by an agent"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "search_notes", "arguments": {"query": "agent"}}}),
    ]
    .iter()
    .map(|m| format!("{m}\n"))
    .collect::<String>();
    let out = run(command, &session);
    assert!(out.status.success(), "{}", stderr(&out));
    let responses: Vec<Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(responses[1]["result"]["tools"].as_array().unwrap().len() >= 10);
    assert_eq!(responses[2]["result"]["isError"], false);
    assert!(
        responses[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Inbox/New.md")
    );
    assert_eq!(
        fs::read_to_string(v.path().join("Inbox/New.md")).unwrap(),
        "made by an agent"
    );
}

fn register_obsidian_vaults(home: &Path, vaults: &[(&Path, bool)]) {
    // Write the settings where Obsidian keeps them on this OS, as the binary sees the environment from `cmd`.
    let os = std::env::consts::OS;
    let config_dir = config_dir_for(os, Some(home.join(".config")), None);
    let config_file = obsidian_config_paths(os, home, config_dir.as_deref()).remove(0);
    fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    let entries: serde_json::Map<String, Value> = vaults
        .iter()
        .enumerate()
        .map(|(i, (path, open))| {
            (
                format!("id{i}"),
                json!({"path": path, "ts": 1, "open": open}),
            )
        })
        .collect();
    fs::write(&config_file, json!({"vaults": entries}).to_string()).unwrap();
}

#[test]
fn vaults_lists_obsidian_vaults_and_names_can_be_used() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    register_obsidian_vaults(home.path(), &[(v.path(), true)]);
    let name = v.path().file_name().unwrap().to_str().unwrap();

    let out = cmd(home.path()).arg("vaults").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(name));

    let out = cmd(home.path())
        .args(["call", "list_notes", "{}", "--vault", name])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    // With a single registered vault, no selector is needed at all.
    let out = cmd(home.path())
        .args(["call", "list_notes"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn setup_writes_client_config_and_keeps_a_backup() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let config = home.path().join(".cursor/mcp.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, r#"{"mcpServers": {"other": {"command": "x"}}}"#).unwrap();

    let out = cmd(home.path())
        .args(["setup", "cursor", "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));

    let written: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(written["mcpServers"]["other"]["command"], "x");
    let entry = &written["mcpServers"]["obsidian"];
    assert!(Path::new(entry["command"].as_str().unwrap()).is_absolute());
    let vault_arg = entry["args"][2].as_str().unwrap();
    assert!(Path::new(vault_arg).is_absolute());
    assert_eq!(
        Path::new(vault_arg).canonicalize().unwrap(),
        v.path().canonicalize().unwrap()
    );
    assert!(home.path().join(".cursor/mcp.json.bak").exists());
}

#[test]
fn setup_print_shows_config_without_writing() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let out = cmd(home.path())
        .args(["setup", "cursor", "--print", "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let printed: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(printed["mcpServers"]["obsidian"].is_object());
    assert!(!home.path().join(".cursor").exists());
}

#[test]
fn setup_generic_and_claude_code_print_instructions() {
    let (home, v) = (TempDir::new().unwrap(), vault());
    let out = cmd(home.path())
        .args(["setup", "generic", "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        serde_json::from_str::<Value>(&stdout(&out)).unwrap()["mcpServers"]["obsidian"].is_object()
    );

    let out = cmd(home.path())
        .args(["setup", "claude-code", "--print", "--vault"])
        .arg(v.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("claude mcp add"));
}
