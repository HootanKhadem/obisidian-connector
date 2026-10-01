//! Generates and installs MCP client configuration so agents can launch the connector.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// The key the connector is registered under in client configs.
pub const SERVER_NAME: &str = "obsidian";

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Client {
    ClaudeDesktop,
    ClaudeCode,
    Cursor,
    Vscode,
    Windsurf,
    /// The `mcpServers` JSON used by most other MCP clients.
    Generic,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SetupError {
    #[error("existing config is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("existing config must be a JSON object")]
    NotAnObject,
    #[error("{0:?} is configured with a command, not a config file")]
    NoConfigFile(Client),
}

/// How the client should launch the connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub binary: PathBuf,
    pub vault: PathBuf,
    pub read_only: bool,
}

impl Launch {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec!["serve".to_string(), "--vault".to_string(), self.vault.display().to_string()];
        if self.read_only {
            args.push("--read-only".to_string());
        }
        args
    }
}

/// The JSON object describing the server for `client`.
pub fn server_entry(client: Client, launch: &Launch) -> Value {
    let mut entry = Map::new();
    if client == Client::Vscode {
        entry.insert("type".into(), json!("stdio"));
    }
    entry.insert("command".into(), json!(launch.binary.display().to_string()));
    entry.insert("args".into(), json!(launch.args()));
    Value::Object(entry)
}

/// The top-level key that holds servers in `client`'s config file.
pub fn servers_key(client: Client) -> &'static str {
    match client {
        Client::Vscode => "servers",
        _ => "mcpServers",
    }
}

/// Inserts (or replaces) the connector in an existing config, keeping every other setting.
pub fn merge_into_config(existing: Option<&str>, client: Client, launch: &Launch) -> Result<String, SetupError> {
    let mut config = match existing.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => serde_json::from_str::<Value>(text).map_err(|e| SetupError::InvalidJson(e.to_string()))?,
        None => json!({}),
    };
    let root = config.as_object_mut().ok_or(SetupError::NotAnObject)?;
    let servers = root
        .entry(servers_key(client))
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or(SetupError::NotAnObject)?;
    servers.insert(SERVER_NAME.to_string(), server_entry(client, launch));
    let mut out = serde_json::to_string_pretty(&config).expect("JSON values always serialize");
    out.push('\n');
    Ok(out)
}

/// A shell command that registers the connector, for clients configured via their own CLI.
pub fn install_command(client: Client, launch: &Launch) -> Option<String> {
    if client != Client::ClaudeCode {
        return None;
    }
    let mut parts = vec![shell_quote(&launch.binary.display().to_string())];
    for arg in launch.args() {
        parts.push(if arg.starts_with('-') || arg == "serve" { arg } else { shell_quote(&arg) });
    }
    Some(format!("claude mcp add --scope user {SERVER_NAME} -- {}", parts.join(" ")))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Where `client` keeps its global MCP config on the given OS.
pub fn config_path(client: Client, os: &str, home: &Path, appdata: Option<&Path>) -> Result<PathBuf, SetupError> {
    // Per-OS folder where desktop apps keep their settings.
    let app_config = || match os {
        "macos" => home.join("Library/Application Support"),
        "windows" => appdata.map(Path::to_path_buf).unwrap_or_else(|| home.join("AppData/Roaming")),
        _ => home.join(".config"),
    };
    match client {
        Client::ClaudeDesktop => Ok(app_config().join("Claude/claude_desktop_config.json")),
        Client::Vscode => Ok(app_config().join("Code/User/mcp.json")),
        Client::Cursor => Ok(home.join(".cursor/mcp.json")),
        Client::Windsurf => Ok(home.join(".codeium/windsurf/mcp_config.json")),
        Client::ClaudeCode | Client::Generic => Err(SetupError::NoConfigFile(client)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(read_only: bool) -> Launch {
        Launch { binary: "/bin/obsidian-connector".into(), vault: "/home/me/My Vault".into(), read_only }
    }

    #[test]
    fn args_point_at_the_vault_and_honour_read_only() {
        assert_eq!(launch(false).args(), vec!["serve", "--vault", "/home/me/My Vault"]);
        assert_eq!(launch(true).args(), vec!["serve", "--vault", "/home/me/My Vault", "--read-only"]);
    }

    #[test]
    fn server_entry_is_a_stdio_command() {
        let entry = server_entry(Client::ClaudeDesktop, &launch(false));
        assert_eq!(entry, json!({"command": "/bin/obsidian-connector", "args": ["serve", "--vault", "/home/me/My Vault"]}));
    }

    #[test]
    fn vscode_entry_declares_its_type() {
        let entry = server_entry(Client::Vscode, &launch(false));
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "/bin/obsidian-connector");
    }

    #[test]
    fn servers_key_differs_for_vscode() {
        assert_eq!(servers_key(Client::Vscode), "servers");
        assert_eq!(servers_key(Client::Cursor), "mcpServers");
    }

    #[test]
    fn merge_creates_a_config_from_nothing() {
        let merged: Value = serde_json::from_str(&merge_into_config(None, Client::Cursor, &launch(false)).unwrap()).unwrap();
        assert_eq!(merged["mcpServers"]["obsidian"]["command"], "/bin/obsidian-connector");
    }

    #[test]
    fn merge_keeps_other_servers_and_settings() {
        let existing = r#"{"theme": "dark", "mcpServers": {"other": {"command": "x"}, "obsidian": {"command": "old"}}}"#;
        let merged: Value =
            serde_json::from_str(&merge_into_config(Some(existing), Client::ClaudeDesktop, &launch(true)).unwrap()).unwrap();
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["mcpServers"]["other"]["command"], "x");
        assert_eq!(merged["mcpServers"]["obsidian"]["command"], "/bin/obsidian-connector");
        assert_eq!(merged["mcpServers"]["obsidian"]["args"][3], "--read-only");
    }

    #[test]
    fn merge_treats_blank_file_as_empty() {
        assert!(merge_into_config(Some("  \n"), Client::Generic, &launch(false)).is_ok());
    }

    #[test]
    fn merge_rejects_invalid_existing_config() {
        assert!(matches!(merge_into_config(Some("{oops"), Client::Cursor, &launch(false)), Err(SetupError::InvalidJson(_))));
        assert_eq!(merge_into_config(Some("[]"), Client::Cursor, &launch(false)), Err(SetupError::NotAnObject));
        assert_eq!(
            merge_into_config(Some(r#"{"mcpServers": 3}"#), Client::Cursor, &launch(false)),
            Err(SetupError::NotAnObject)
        );
    }

    #[test]
    fn claude_code_uses_its_cli() {
        assert_eq!(
            install_command(Client::ClaudeCode, &launch(false)).unwrap(),
            "claude mcp add --scope user obsidian -- '/bin/obsidian-connector' serve --vault '/home/me/My Vault'"
        );
        assert_eq!(install_command(Client::Cursor, &launch(false)), None);
    }

    #[test]
    fn install_command_quotes_single_quotes() {
        let l = Launch { binary: "/bin/oc".into(), vault: "/v/it's".into(), read_only: false };
        assert!(install_command(Client::ClaudeCode, &l).unwrap().ends_with(r#"--vault '/v/it'\''s'"#));
    }

    #[test]
    fn config_paths_per_client_and_os() {
        let home = Path::new("/home/me");
        assert_eq!(
            config_path(Client::ClaudeDesktop, "macos", Path::new("/Users/me"), None).unwrap(),
            PathBuf::from("/Users/me/Library/Application Support/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(Client::ClaudeDesktop, "windows", home, Some(Path::new("C:/AppData"))).unwrap(),
            PathBuf::from("C:/AppData/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(Client::ClaudeDesktop, "linux", home, None).unwrap(),
            PathBuf::from("/home/me/.config/Claude/claude_desktop_config.json")
        );
        assert_eq!(config_path(Client::Cursor, "linux", home, None).unwrap(), PathBuf::from("/home/me/.cursor/mcp.json"));
        assert_eq!(
            config_path(Client::Windsurf, "linux", home, None).unwrap(),
            PathBuf::from("/home/me/.codeium/windsurf/mcp_config.json")
        );
        assert_eq!(
            config_path(Client::Vscode, "macos", Path::new("/Users/me"), None).unwrap(),
            PathBuf::from("/Users/me/Library/Application Support/Code/User/mcp.json")
        );
        assert_eq!(
            config_path(Client::Vscode, "linux", home, None).unwrap(),
            PathBuf::from("/home/me/.config/Code/User/mcp.json")
        );
        assert_eq!(config_path(Client::ClaudeCode, "linux", home, None), Err(SetupError::NoConfigFile(Client::ClaudeCode)));
        assert_eq!(config_path(Client::Generic, "linux", home, None), Err(SetupError::NoConfigFile(Client::Generic)));
    }
}
