//! End-to-end tests that drive the compiled binary the way agents and users do.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use obsidian_connector::discovery::{config_directory_for, obsidian_config_paths};
use serde_json::{Value, json};
use tempfile::TempDir;

fn sample_vault() -> TempDir {
    let vault_directory = TempDir::new().unwrap();
    fs::write(
        vault_directory.path().join("Hello.md"),
        "Hello [[World]] #greeting",
    )
    .unwrap();
    fs::write(vault_directory.path().join("World.md"), "The world").unwrap();
    vault_directory
}

/// A command isolated from the developer's real Obsidian and client settings.
fn connector_command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_obsidian-connector"));
    command
        .env_remove("OBSIDIAN_VAULT")
        .env_remove("APPDATA")
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join(".config"));
    command
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
    let output = connector_command(home.path())
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn tools_exports_schemas_without_a_vault() {
    let home = TempDir::new().unwrap();
    let output = connector_command(home.path())
        .args(["tools", "--format", "openai"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let tools: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(tools[0]["type"], "function");

    let output = connector_command(home.path())
        .args(["tools", "--read-only"])
        .output()
        .unwrap();
    let tools: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["name"] != "delete_note")
    );
}

#[test]
fn call_runs_a_tool_and_prints_its_result() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let output = connector_command(home.path())
        .args(["call", "read_note", r#"{"path": "Hello"}"#, "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim_end(), "Hello [[World]] #greeting");
}

#[test]
fn call_reads_arguments_from_stdin_and_vault_from_env() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let mut command = connector_command(home.path());
    command
        .args(["call", "get_backlinks", "-"])
        .env("OBSIDIAN_VAULT", vault.path());
    let output = run(command, r#"{"path": "World"}"#);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap(),
        json!(["Hello.md"])
    );
}

#[test]
fn call_without_arguments_uses_an_empty_object() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let output = connector_command(home.path())
        .args(["call", "list_notes", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap(),
        json!(["Hello.md", "World.md"])
    );
}

#[test]
fn call_failures_exit_non_zero_with_a_message() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let output = connector_command(home.path())
        .args(["call", "read_note", r#"{"path": "Missing"}"#, "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("not found"));

    let output = connector_command(home.path())
        .args(["call", "read_note", "{bad", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("JSON"));
}

#[test]
fn read_only_blocks_writes() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let output = connector_command(home.path())
        .args([
            "call",
            "delete_note",
            r#"{"path": "Hello"}"#,
            "--read-only",
            "--vault",
        ])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(vault.path().join("Hello.md").exists());
}

#[test]
fn missing_vault_is_explained() {
    let home = TempDir::new().unwrap();
    let output = connector_command(home.path())
        .args(["call", "list_notes"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--vault"), "{}", stderr(&output));
}

#[test]
fn serve_speaks_mcp_over_stdio() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let mut command = connector_command(home.path());
    command.args(["serve", "--vault"]).arg(vault.path());
    let session = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "create_note", "arguments": {"path": "Inbox/New", "content": "made by an agent"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "search_notes", "arguments": {"query": "agent"}}}),
    ]
    .iter()
    .map(|message| format!("{message}\n"))
    .collect::<String>();
    let output = run(command, &session);
    assert!(output.status.success(), "{}", stderr(&output));
    let responses: Vec<Value> = stdout(&output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
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
        fs::read_to_string(vault.path().join("Inbox/New.md")).unwrap(),
        "made by an agent"
    );
}

fn register_obsidian_vaults(home: &Path, vaults: &[(&Path, bool)]) {
    // Write the settings where Obsidian keeps them on this OS, as the binary sees the environment from `cmd`.
    let os = std::env::consts::OS;
    let config_directory = config_directory_for(os, Some(home.join(".config")), None);
    let config_file = obsidian_config_paths(os, home, config_directory.as_deref()).remove(0);
    fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    let entries: serde_json::Map<String, Value> = vaults
        .iter()
        .enumerate()
        .map(|(index, (path, open))| {
            (
                format!("id{index}"),
                json!({"path": path, "ts": 1, "open": open}),
            )
        })
        .collect();
    fs::write(&config_file, json!({"vaults": entries}).to_string()).unwrap();
}

#[test]
fn vaults_lists_obsidian_vaults_and_names_can_be_used() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    register_obsidian_vaults(home.path(), &[(vault.path(), true)]);
    let name = vault.path().file_name().unwrap().to_str().unwrap();

    let output = connector_command(home.path())
        .arg("vaults")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains(name));

    let output = connector_command(home.path())
        .args(["call", "list_notes", "{}", "--vault", name])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    // With a single registered vault, no selector is needed at all.
    let output = connector_command(home.path())
        .args(["call", "list_notes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn setup_writes_client_config_and_keeps_a_backup() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let config = home.path().join(".cursor/mcp.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, r#"{"mcpServers": {"other": {"command": "x"}}}"#).unwrap();

    let output = connector_command(home.path())
        .args(["setup", "cursor", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let written: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(written["mcpServers"]["other"]["command"], "x");
    let entry = &written["mcpServers"]["obsidian"];
    assert!(Path::new(entry["command"].as_str().unwrap()).is_absolute());
    let vault_arg = entry["args"][2].as_str().unwrap();
    assert!(Path::new(vault_arg).is_absolute());
    assert_eq!(
        Path::new(vault_arg).canonicalize().unwrap(),
        vault.path().canonicalize().unwrap()
    );
    assert!(home.path().join(".cursor/mcp.json.bak").exists());
}

#[test]
fn setup_print_shows_config_without_writing() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let output = connector_command(home.path())
        .args(["setup", "cursor", "--print", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let printed: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(printed["mcpServers"]["obsidian"].is_object());
    assert!(!home.path().join(".cursor").exists());
}

#[test]
fn setup_generic_and_claude_code_print_instructions() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let output = connector_command(home.path())
        .args(["setup", "generic", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        serde_json::from_str::<Value>(&stdout(&output)).unwrap()["mcpServers"]["obsidian"]
            .is_object()
    );

    let output = connector_command(home.path())
        .args(["setup", "claude-code", "--print", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("claude mcp add"));
}

/// Puts a fake `claude` CLI first on PATH that records its arguments and exits with `exit_code`.
#[cfg(unix)]
fn fake_claude_cli(exit_code: i32) -> (TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin_directory = TempDir::new().unwrap();
    let recorded_arguments = bin_directory.path().join("arguments.txt");
    let script = bin_directory.path().join("claude");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$@\" > '{}'\nexit {exit_code}\n",
            recorded_arguments.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (bin_directory, recorded_arguments)
}

#[cfg(unix)]
fn path_with(directory: &Path) -> std::ffi::OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(
        std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&existing)),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn setup_claude_code_registers_through_the_claude_cli() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let (bin_directory, recorded_arguments) = fake_claude_cli(0);
    let output = connector_command(home.path())
        .env("PATH", path_with(bin_directory.path()))
        .args(["setup", "claude-code", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let arguments = fs::read_to_string(recorded_arguments).unwrap();
    assert!(
        arguments.starts_with("mcp add --scope user obsidian -- "),
        "{arguments}"
    );
    assert!(arguments.trim_end().ends_with(&format!(
        "serve --vault {}",
        vault.path().canonicalize().unwrap().display()
    )));
}

#[cfg(unix)]
#[test]
fn setup_claude_code_reports_a_failing_claude_cli() {
    let (home, vault) = (TempDir::new().unwrap(), sample_vault());
    let (bin_directory, _) = fake_claude_cli(1);
    let output = connector_command(home.path())
        .env("PATH", path_with(bin_directory.path()))
        .args(["setup", "claude-code", "--vault"])
        .arg(vault.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("claude mcp add"),
        "{}",
        stderr(&output)
    );
}
