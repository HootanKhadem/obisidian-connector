use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde_json::Value;

use obsidian_connector::discovery::{known_vaults, resolve_vault};
use obsidian_connector::mcp::Server;
use obsidian_connector::setup::{self, Client, Launch};
use obsidian_connector::tools::{self, SchemaFormat, Toolbox};
use obsidian_connector::vault::Vault;

/// Connect any AI agent to an Obsidian vault, via MCP or plain JSON on the command line.
#[derive(Parser)]
#[command(name = "obsidian-connector", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct VaultOptions {
    /// Vault folder or Obsidian vault name. Defaults to the only (or currently open) vault in Obsidian.
    #[arg(long, short, env = "OBSIDIAN_VAULT")]
    vault: Option<String>,
    /// Only expose tools that read the vault.
    #[arg(long)]
    read_only: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server on stdin/stdout (this is what MCP clients launch).
    Serve(VaultOptions),
    /// Run a single tool and print its result, e.g. `call search_notes '{"query": "rust"}'`.
    Call {
        /// Tool name (see `tools`).
        tool: String,
        /// Tool arguments as a JSON object, or `-` to read them from stdin.
        arguments: Option<String>,
        #[command(flatten)]
        options: VaultOptions,
    },
    /// Print the tool schemas for MCP, OpenAI or Anthropic function calling.
    Tools {
        #[arg(long, value_enum, default_value = "mcp")]
        format: SchemaFormat,
        /// Only list tools that read the vault.
        #[arg(long)]
        read_only: bool,
    },
    /// List the vaults registered in the Obsidian app.
    Vaults,
    /// Register the connector with an MCP client such as Claude Desktop or Cursor.
    Setup {
        #[arg(value_enum)]
        client: Client,
        #[command(flatten)]
        options: VaultOptions,
        /// Print the configuration instead of installing it.
        #[arg(long)]
        print: bool,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Serve(options) => {
            let vault = open_vault(options.vault.as_deref())?;
            eprintln!(
                "obsidian-connector: serving vault {}",
                vault.root().display()
            );
            let server = Server::new(Toolbox::new(vault, options.read_only));
            server
                .run(std::io::stdin().lock(), std::io::stdout().lock())
                .map_err(|error| error.to_string())
        }
        Command::Call {
            tool,
            arguments,
            options,
        } => {
            let arguments = parse_arguments(arguments.as_deref())?;
            let toolbox = Toolbox::new(open_vault(options.vault.as_deref())?, options.read_only);
            let output = toolbox
                .call(&tool, &arguments)
                .map_err(|error| error.to_string())?;
            println!("{output}");
            Ok(())
        }
        Command::Tools { format, read_only } => {
            let schemas = tools::export(format, read_only);
            println!(
                "{}",
                serde_json::to_string_pretty(&schemas).expect("JSON values always serialize")
            );
            Ok(())
        }
        Command::Vaults => {
            let vaults = known_vaults();
            if vaults.is_empty() {
                eprintln!("No vaults found in Obsidian's settings. Pass --vault <folder> instead.");
            }
            for vault in vaults {
                let open = if vault.open { "  (open)" } else { "" };
                println!("{}\t{}{open}", vault.name, vault.path.display());
            }
            Ok(())
        }
        Command::Setup {
            client,
            options,
            print,
        } => install(client, &options, print),
    }
}

fn open_vault(selector: Option<&str>) -> Result<Vault, String> {
    let path = resolve_vault(selector, &known_vaults()).map_err(|error| error.to_string())?;
    Vault::open(path).map_err(|error| error.to_string())
}

fn parse_arguments(raw: Option<&str>) -> Result<Value, String> {
    let text = match raw {
        None => return Ok(Value::Object(Default::default())),
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|error| error.to_string())?;
            buf
        }
        Some(text) => text.to_string(),
    };
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(&text).map_err(|error| format!("arguments must be a JSON object: {error}"))
}

fn install(client: Client, options: &VaultOptions, print: bool) -> Result<(), String> {
    let vault = open_vault(options.vault.as_deref())?;
    let binary = std::env::current_exe().map_err(|error| error.to_string())?;
    let launch = Launch {
        binary: clean_path(&binary),
        vault: clean_path(vault.root()),
        read_only: options.read_only,
    };

    if client == Client::ClaudeCode {
        let command =
            setup::install_command(client, &launch).expect("Claude Code is configured by command");
        if print {
            println!("{command}");
            return Ok(());
        }
        let status = std::process::Command::new("claude")
            .args(["mcp", "add", "--scope", "user", setup::SERVER_NAME, "--"])
            .arg(&launch.binary)
            .args(launch.arguments())
            .status();
        return match status {
            Ok(exit_status) if exit_status.success() => Ok(()),
            _ => Err(format!(
                "could not run the `claude` CLI; run this yourself:\n  {command}"
            )),
        };
    }

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let path = match (print, home) {
        (false, Some(home)) => {
            setup::config_path(client, std::env::consts::OS, &home, appdata.as_deref()).ok()
        }
        _ => None,
    };
    let Some(path) = path else {
        print!(
            "{}",
            setup::merge_into_config(None, client, &launch).map_err(|error| error.to_string())?
        );
        return Ok(());
    };

    let existing = std::fs::read_to_string(&path).ok();
    let merged = setup::merge_into_config(existing.as_deref(), client, &launch)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if existing.is_some() {
        let backup = PathBuf::from(format!("{}.bak", path.display()));
        std::fs::copy(&path, &backup).map_err(|error| error.to_string())?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&path, merged).map_err(|error| error.to_string())?;
    println!(
        "Added the '{}' MCP server to {}.",
        setup::SERVER_NAME,
        path.display()
    );
    println!("Restart the app to load it.");
    Ok(())
}

/// An absolute path without Windows' `\\?\` verbatim prefix, which some clients cannot launch.
fn clean_path(path: &Path) -> PathBuf {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match absolute.to_str().and_then(strip_verbatim_prefix) {
        Some(rest) => PathBuf::from(rest),
        None => absolute,
    }
}

fn strip_verbatim_prefix(path: &str) -> Option<&str> {
    path.strip_prefix(r"\\?\")
        .filter(|rest| !rest.starts_with("UNC"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_arguments_defaults_to_an_empty_object() {
        assert_eq!(parse_arguments(None).unwrap(), json!({}));
        assert_eq!(parse_arguments(Some("  ")).unwrap(), json!({}));
    }

    #[test]
    fn parse_arguments_parses_json_and_reports_errors() {
        assert_eq!(
            parse_arguments(Some(r#"{"a": 1}"#)).unwrap(),
            json!({"a": 1})
        );
        assert!(parse_arguments(Some("{oops")).unwrap_err().contains("JSON"));
    }

    #[test]
    fn strip_verbatim_prefix_handles_windows_paths() {
        assert_eq!(
            strip_verbatim_prefix(r"\\?\C:\Users\me"),
            Some(r"C:\Users\me")
        );
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\server\share"), None);
        assert_eq!(strip_verbatim_prefix("/home/me"), None);
    }

    #[test]
    fn clean_path_makes_existing_paths_absolute() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(clean_path(dir.path()).is_absolute());
        assert_eq!(
            clean_path(Path::new("/does/not/exist")),
            PathBuf::from("/does/not/exist")
        );
    }
}
