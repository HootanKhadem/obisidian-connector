//! Finds vaults registered with the Obsidian app so users can refer to them by name.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownVault {
    pub name: String,
    pub path: PathBuf,
    pub open: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error(
        "no vault given and none found in Obsidian's settings; pass --vault <path> or set OBSIDIAN_VAULT"
    )]
    NoVault,
    #[error("several Obsidian vaults found ({}); choose one with --vault <name or path>", .0.join(", "))]
    Ambiguous(Vec<String>),
    #[error("'{name}' is neither a folder nor a known Obsidian vault (known vaults: {known})", name = .0, known = .1.join(", "))]
    Unknown(String, Vec<String>),
}

/// Parses Obsidian's `obsidian.json`, which lists every vault the app knows about.
pub fn parse_obsidian_config(json: &str) -> Vec<KnownVault> {
    let Ok(config) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let Some(vaults) = config.get("vaults").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<KnownVault> = vaults
        .values()
        .filter_map(|vault_entry| {
            let path = vault_entry.get("path")?.as_str()?;
            let name = path
                .rsplit(['/', '\\'])
                .find(|segment| !segment.is_empty())?;
            Some(KnownVault {
                name: name.to_string(),
                path: PathBuf::from(path),
                open: vault_entry
                    .get("open")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect();
    found.sort_by(|first, second| first.name.cmp(&second.name));
    found
}

/// Locations where Obsidian stores `obsidian.json` on the given OS (`std::env::consts::OS` values).
pub fn obsidian_config_paths(
    os: &str,
    home: &Path,
    config_directory: Option<&Path>,
) -> Vec<PathBuf> {
    let file = Path::new("obsidian").join("obsidian.json");
    match os {
        "macos" => vec![home.join("Library/Application Support").join(&file)],
        "windows" => {
            let appdata = config_directory
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.join("AppData/Roaming"));
            vec![appdata.join(&file)]
        }
        _ => {
            let config = config_directory
                .map(Path::to_path_buf)
                .unwrap_or_else(|| home.join(".config"));
            vec![
                config.join(&file),
                home.join(".var/app/md.obsidian.Obsidian/config")
                    .join(&file),
                home.join("snap/obsidian/current/.config").join(&file),
            ]
        }
    }
}

/// The settings folder Obsidian uses, from `XDG_CONFIG_HOME` on Linux or `APPDATA` on Windows.
pub fn config_directory_for(
    os: &str,
    xdg_config_home: Option<PathBuf>,
    appdata: Option<PathBuf>,
) -> Option<PathBuf> {
    match os {
        "windows" => appdata,
        "macos" => None,
        _ => xdg_config_home,
    }
}

/// Picks the vault to serve from an explicit selector (path or vault name) or the known vaults.
pub fn resolve_vault(
    selector: Option<&str>,
    known: &[KnownVault],
) -> Result<PathBuf, DiscoveryError> {
    let names = || {
        known
            .iter()
            .map(|vault| vault.name.clone())
            .collect::<Vec<_>>()
    };
    if let Some(selector) = selector {
        if Path::new(selector).is_dir() {
            return Ok(PathBuf::from(selector));
        }
        return known
            .iter()
            .find(|vault| vault.name.eq_ignore_ascii_case(selector))
            .map(|vault| vault.path.clone())
            .ok_or_else(|| DiscoveryError::Unknown(selector.to_string(), names()));
    }
    match known {
        [] => Err(DiscoveryError::NoVault),
        [only] => Ok(only.path.clone()),
        _ => {
            let open: Vec<_> = known.iter().filter(|vault| vault.open).collect();
            match open.as_slice() {
                [only] => Ok(only.path.clone()),
                _ => Err(DiscoveryError::Ambiguous(names())),
            }
        }
    }
}

/// Reads every vault registered in Obsidian on this machine.
pub fn known_vaults() -> Vec<KnownVault> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let config_directory = config_directory_for(
        std::env::consts::OS,
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
    );
    obsidian_config_paths(std::env::consts::OS, &home, config_directory.as_deref())
        .iter()
        .filter_map(|config_file| std::fs::read_to_string(config_file).ok())
        .flat_map(|json| parse_obsidian_config(&json))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn known(name: &str, path: &str, open: bool) -> KnownVault {
        KnownVault {
            name: name.into(),
            path: PathBuf::from(path),
            open,
        }
    }

    #[test]
    fn parse_obsidian_config_reads_vaults_sorted_by_name() {
        let json = r#"{"vaults": {
            "abc": {"path": "/home/me/Notes", "ts": 1, "open": true},
            "def": {"path": "C:\\Users\\me\\Work Vault", "ts": 2}
        }}"#;
        assert_eq!(
            parse_obsidian_config(json),
            vec![
                known("Notes", "/home/me/Notes", true),
                known("Work Vault", "C:\\Users\\me\\Work Vault", false)
            ]
        );
    }

    #[test]
    fn parse_obsidian_config_tolerates_garbage() {
        assert!(parse_obsidian_config("not json").is_empty());
        assert!(parse_obsidian_config("{}").is_empty());
        assert!(parse_obsidian_config(r#"{"vaults": {"x": {"ts": 1}}}"#).is_empty());
    }

    #[test]
    fn config_paths_for_linux_include_flatpak_and_snap() {
        let paths = obsidian_config_paths("linux", Path::new("/home/me"), None);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/me/.config/obsidian/obsidian.json"),
                PathBuf::from(
                    "/home/me/.var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json"
                ),
                PathBuf::from("/home/me/snap/obsidian/current/.config/obsidian/obsidian.json"),
            ]
        );
        let custom = obsidian_config_paths("linux", Path::new("/home/me"), Some(Path::new("/xdg")));
        assert_eq!(custom[0], PathBuf::from("/xdg/obsidian/obsidian.json"));
    }

    #[test]
    fn config_paths_for_macos_and_windows() {
        assert_eq!(
            obsidian_config_paths("macos", Path::new("/Users/me"), None),
            vec![PathBuf::from(
                "/Users/me/Library/Application Support/obsidian/obsidian.json"
            )]
        );
        assert_eq!(
            obsidian_config_paths(
                "windows",
                Path::new("C:/Users/me"),
                Some(Path::new("C:/Users/me/AppData/Roaming"))
            ),
            vec![PathBuf::from(
                "C:/Users/me/AppData/Roaming/obsidian/obsidian.json"
            )]
        );
        assert_eq!(
            obsidian_config_paths("windows", Path::new("C:/Users/me"), None),
            vec![PathBuf::from(
                "C:/Users/me/AppData/Roaming/obsidian/obsidian.json"
            )]
        );
    }

    #[test]
    fn config_dir_uses_the_variable_for_the_os() {
        let xdg = || Some(PathBuf::from("/xdg"));
        let appdata = || Some(PathBuf::from("C:/AppData"));
        assert_eq!(config_directory_for("linux", xdg(), appdata()), xdg());
        assert_eq!(config_directory_for("windows", xdg(), appdata()), appdata());
        assert_eq!(config_directory_for("windows", xdg(), None), None);
        assert_eq!(config_directory_for("macos", xdg(), appdata()), None);
    }

    #[test]
    fn resolve_prefers_an_existing_folder() {
        let vault_directory = TempDir::new().unwrap();
        let selector = vault_directory.path().to_str().unwrap();
        assert_eq!(
            resolve_vault(Some(selector), &[known("Other", "/x", true)]).unwrap(),
            vault_directory.path()
        );
    }

    #[test]
    fn resolve_matches_vault_names_case_insensitively() {
        let vaults = [known("Notes", "/n", false), known("Work", "/w", false)];
        assert_eq!(
            resolve_vault(Some("work"), &vaults).unwrap(),
            PathBuf::from("/w")
        );
    }

    #[test]
    fn resolve_reports_unknown_selector() {
        let vaults = [known("Notes", "/n", false)];
        assert_eq!(
            resolve_vault(Some("/definitely/missing"), &vaults),
            Err(DiscoveryError::Unknown(
                "/definitely/missing".into(),
                vec!["Notes".into()]
            ))
        );
    }

    #[test]
    fn resolve_without_selector_uses_the_only_or_the_open_vault() {
        assert_eq!(
            resolve_vault(None, &[known("Notes", "/n", false)]).unwrap(),
            PathBuf::from("/n")
        );
        let vaults = [known("A", "/a", false), known("B", "/b", true)];
        assert_eq!(resolve_vault(None, &vaults).unwrap(), PathBuf::from("/b"));
    }

    #[test]
    fn resolve_without_selector_errors_when_ambiguous_or_empty() {
        assert_eq!(resolve_vault(None, &[]), Err(DiscoveryError::NoVault));
        let vaults = [known("A", "/a", false), known("B", "/b", false)];
        assert_eq!(
            resolve_vault(None, &vaults),
            Err(DiscoveryError::Ambiguous(vec!["A".into(), "B".into()]))
        );
    }
}
