//! Safe, sandboxed access to the notes of an Obsidian vault on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault directory not found: {0}")]
    VaultNotFound(PathBuf),
    #[error("note not found: {0}")]
    NotFound(String),
    #[error("note already exists: {0} (set overwrite to replace it)")]
    AlreadyExists(String),
    #[error("invalid note path '{0}': {1}")]
    InvalidPath(String, &'static str),
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, VaultError>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchMatch {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub path: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteInfo {
    pub path: String,
    pub frontmatter: Map<String, Value>,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub backlinks: Vec<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(VaultError::VaultNotFound(path.to_path_buf()));
        }
        Ok(Self {
            root: path.canonicalize()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Normalises a vault-relative note path (forward slashes, `.md` extension) and rejects escapes.
    pub fn normalize(&self, note: &str) -> Result<String> {
        let mut rel = normalize_segments(note)?;
        if rel.is_empty() || note.trim().ends_with(['/', '\\']) {
            return Err(VaultError::InvalidPath(
                note.to_string(),
                "a note name is required",
            ));
        }
        if !rel.to_lowercase().ends_with(".md") {
            rel.push_str(".md");
        }
        Ok(rel)
    }

    /// Resolves a normalised relative path to a location that is guaranteed to stay inside the vault.
    fn full_path(&self, original: &str, rel: &str) -> Result<PathBuf> {
        let full = self.root.join(rel);
        let mut existing = full.as_path();
        while !existing.exists() {
            existing = existing.parent().unwrap_or(&self.root);
        }
        if !existing.canonicalize()?.starts_with(&self.root) {
            return Err(VaultError::InvalidPath(
                original.to_string(),
                "path leaves the vault",
            ));
        }
        Ok(full)
    }

    fn existing_note(&self, note: &str) -> Result<(String, PathBuf)> {
        let rel = self.normalize(note)?;
        let full = self.full_path(note, &rel)?;
        if !full.is_file() {
            return Err(VaultError::NotFound(rel));
        }
        Ok((rel, full))
    }

    pub fn list_notes(&self, folder: Option<&str>) -> Result<Vec<String>> {
        let base = match folder {
            Some(folder) => {
                let rel = normalize_segments(folder)?;
                let full = self.full_path(folder, &rel)?;
                if !full.is_dir() {
                    return Err(VaultError::NotFound(rel));
                }
                full
            }
            None => self.root.clone(),
        };
        let mut notes = Vec::new();
        let walker = walkdir::WalkDir::new(&base)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
        for entry in walker {
            let entry = entry.map_err(|e| std::io::Error::other(e.to_string()))?;
            let is_md = entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
            if entry.file_type().is_file()
                && is_md
                && let Ok(rel) = entry.path().strip_prefix(&self.root)
            {
                let parts: Vec<_> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect();
                notes.push(parts.join("/"));
            }
        }
        notes.sort();
        Ok(notes)
    }

    pub fn read_note(&self, note: &str) -> Result<String> {
        let (_, full) = self.existing_note(note)?;
        Ok(std::fs::read_to_string(full)?)
    }

    pub fn write_note(&self, note: &str, content: &str, overwrite: bool) -> Result<String> {
        let rel = self.normalize(note)?;
        let full = self.full_path(note, &rel)?;
        if full.exists() && !overwrite {
            return Err(VaultError::AlreadyExists(rel));
        }
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(full, content)?;
        Ok(rel)
    }

    pub fn append_note(&self, note: &str, content: &str) -> Result<String> {
        let rel = self.normalize(note)?;
        let full = self.full_path(note, &rel)?;
        let mut existing = if full.is_file() {
            std::fs::read_to_string(&full)?
        } else {
            String::new()
        };
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(content);
        self.write_note(&rel, &existing, true)
    }

    pub fn delete_note(&self, note: &str) -> Result<String> {
        let (rel, full) = self.existing_note(note)?;
        std::fs::remove_file(full)?;
        Ok(rel)
    }

    pub fn move_note(&self, from: &str, to: &str) -> Result<String> {
        let (_, source) = self.existing_note(from)?;
        let target_rel = self.normalize(to)?;
        let target = self.full_path(to, &target_rel)?;
        if target.exists() {
            return Err(VaultError::AlreadyExists(target_rel));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(source, target)?;
        Ok(target_rel)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if terms.is_empty() {
            return Err(VaultError::InvalidInput("search query is empty"));
        }
        let mut results = Vec::new();
        for path in self.list_notes(None)? {
            if results.len() >= limit {
                break;
            }
            let content = std::fs::read_to_string(self.root.join(&path)).unwrap_or_default();
            let haystack = format!("{}\n{}", path.to_lowercase(), content.to_lowercase());
            if !terms.iter().all(|t| haystack.contains(t)) {
                continue;
            }
            let matches = content
                .lines()
                .enumerate()
                .filter(|(_, line)| {
                    let lower = line.to_lowercase();
                    terms.iter().any(|t| lower.contains(t))
                })
                .map(|(i, line)| SearchMatch {
                    line: i + 1,
                    text: line.trim_end().to_string(),
                })
                .collect();
            results.push(SearchResult { path, matches });
        }
        Ok(results)
    }

    pub fn backlinks(&self, note: &str) -> Result<Vec<String>> {
        let (rel, _) = self.existing_note(note)?;
        let target = strip_md(&rel.to_lowercase()).to_string();
        let mut sources = Vec::new();
        for path in self.list_notes(None)? {
            if path == rel {
                continue;
            }
            let content = std::fs::read_to_string(self.root.join(&path)).unwrap_or_default();
            let links_here = crate::markdown::extract_wikilinks(&content)
                .iter()
                .any(|link| {
                    let link = link.replace('\\', "/").to_lowercase();
                    let link = strip_md(link.trim_start_matches('/'));
                    target == link || target.ends_with(&format!("/{link}"))
                });
            if links_here {
                sources.push(path);
            }
        }
        Ok(sources)
    }

    pub fn tags(&self) -> Result<BTreeMap<String, usize>> {
        let mut counts = BTreeMap::new();
        for path in self.list_notes(None)? {
            let content = std::fs::read_to_string(self.root.join(&path)).unwrap_or_default();
            for tag in crate::markdown::extract_tags(&content) {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    pub fn notes_with_tag(&self, tag: &str) -> Result<Vec<String>> {
        let wanted = tag.trim().trim_start_matches('#').to_lowercase();
        let nested = format!("{wanted}/");
        let mut notes = Vec::new();
        for path in self.list_notes(None)? {
            let content = std::fs::read_to_string(self.root.join(&path)).unwrap_or_default();
            let has_tag = crate::markdown::extract_tags(&content).iter().any(|t| {
                let t = t.to_lowercase();
                t == wanted || t.starts_with(&nested)
            });
            if has_tag {
                notes.push(path);
            }
        }
        Ok(notes)
    }

    pub fn note_info(&self, note: &str) -> Result<NoteInfo> {
        let (rel, full) = self.existing_note(note)?;
        let content = std::fs::read_to_string(&full)?;
        Ok(NoteInfo {
            frontmatter: crate::markdown::parse_frontmatter(&content),
            tags: crate::markdown::extract_tags(&content),
            links: crate::markdown::extract_wikilinks(&content),
            backlinks: self.backlinks(&rel)?,
            size_bytes: std::fs::metadata(&full)?.len(),
            path: rel,
        })
    }
}

fn strip_md(path: &str) -> &str {
    path.strip_suffix(".md").unwrap_or(path)
}

/// Splits a user-supplied relative path into safe segments joined by `/`.
fn normalize_segments(raw: &str) -> Result<String> {
    let invalid = |reason| Err(VaultError::InvalidPath(raw.to_string(), reason));
    let cleaned = raw.trim().replace('\\', "/");
    let mut segments = Vec::new();
    for segment in cleaned.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return invalid("parent directory references are not allowed"),
            s if s.starts_with('.') => {
                return invalid("hidden files and folders are not accessible");
            }
            s if s.contains(':') => return invalid("absolute paths are not allowed"),
            s => segments.push(s),
        }
    }
    Ok(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn vault_with(files: &[(&str, &str)]) -> (TempDir, Vault) {
        let dir = TempDir::new().unwrap();
        for (path, content) in files {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn open_fails_for_missing_directory() {
        let dir = TempDir::new().unwrap();
        let err = Vault::open(dir.path().join("missing")).unwrap_err();
        assert!(matches!(err, VaultError::VaultNotFound(_)));
    }

    #[test]
    fn open_fails_for_a_file() {
        let (dir, _) = vault_with(&[("a.md", "")]);
        assert!(Vault::open(dir.path().join("a.md")).is_err());
    }

    #[test]
    fn normalize_adds_extension_and_uses_forward_slashes() {
        let (_dir, vault) = vault_with(&[]);
        assert_eq!(vault.normalize("Folder/Note").unwrap(), "Folder/Note.md");
        assert_eq!(
            vault.normalize("Folder\\Note.md").unwrap(),
            "Folder/Note.md"
        );
        assert_eq!(vault.normalize("./v1.2 notes").unwrap(), "v1.2 notes.md");
        assert_eq!(
            vault.normalize("/leading/slash").unwrap(),
            "leading/slash.md"
        );
    }

    #[test]
    fn normalize_rejects_escapes_hidden_and_empty_paths() {
        let (_dir, vault) = vault_with(&[]);
        for bad in [
            "../outside",
            "a/../../b",
            ".obsidian/app",
            "a/.git/x",
            "",
            "   ",
            "folder/",
        ] {
            assert!(
                matches!(vault.normalize(bad), Err(VaultError::InvalidPath(..))),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape_the_vault() {
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret.md"), "secret").unwrap();
        let (dir, vault) = vault_with(&[]);
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert!(matches!(
            vault.read_note("link/secret"),
            Err(VaultError::InvalidPath(..))
        ));
        assert!(matches!(
            vault.write_note("link/new", "x", false),
            Err(VaultError::InvalidPath(..))
        ));
    }

    #[test]
    fn list_notes_returns_sorted_markdown_and_skips_hidden() {
        let (_dir, vault) = vault_with(&[
            ("b.md", ""),
            ("A/c.md", ""),
            ("A/image.png", ""),
            (".obsidian/workspace.md", ""),
            (".trash/old.md", ""),
        ]);
        assert_eq!(vault.list_notes(None).unwrap(), vec!["A/c.md", "b.md"]);
    }

    #[test]
    fn list_notes_filters_by_folder() {
        let (_dir, vault) = vault_with(&[
            ("A/c.md", ""),
            ("A/B/d.md", ""),
            ("AB/e.md", ""),
            ("f.md", ""),
        ]);
        assert_eq!(
            vault.list_notes(Some("A")).unwrap(),
            vec!["A/B/d.md", "A/c.md"]
        );
        assert!(matches!(
            vault.list_notes(Some("missing")),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn read_note_returns_content_and_errors_when_missing() {
        let (_dir, vault) = vault_with(&[("Note.md", "hello")]);
        assert_eq!(vault.read_note("Note").unwrap(), "hello");
        assert!(matches!(
            vault.read_note("Nope"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn write_note_creates_folders_and_refuses_to_overwrite() {
        let (dir, vault) = vault_with(&[]);
        assert_eq!(
            vault.write_note("New/Deep/Note", "one", false).unwrap(),
            "New/Deep/Note.md"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("New/Deep/Note.md")).unwrap(),
            "one"
        );
        assert!(matches!(
            vault.write_note("New/Deep/Note", "two", false),
            Err(VaultError::AlreadyExists(_))
        ));
        vault.write_note("New/Deep/Note", "two", true).unwrap();
        assert_eq!(vault.read_note("New/Deep/Note").unwrap(), "two");
    }

    #[test]
    fn append_note_adds_a_newline_separator_and_creates_missing_notes() {
        let (_dir, vault) = vault_with(&[("Log.md", "first")]);
        vault.append_note("Log", "second").unwrap();
        assert_eq!(vault.read_note("Log").unwrap(), "first\nsecond");
        vault.append_note("Fresh", "only").unwrap();
        assert_eq!(vault.read_note("Fresh").unwrap(), "only");
    }

    #[test]
    fn append_note_does_not_double_newlines() {
        let (_dir, vault) = vault_with(&[("Log.md", "first\n")]);
        vault.append_note("Log", "second").unwrap();
        assert_eq!(vault.read_note("Log").unwrap(), "first\nsecond");
    }

    #[test]
    fn delete_note_removes_file() {
        let (dir, vault) = vault_with(&[("Gone.md", "x")]);
        assert_eq!(vault.delete_note("Gone").unwrap(), "Gone.md");
        assert!(!dir.path().join("Gone.md").exists());
        assert!(matches!(
            vault.delete_note("Gone"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn move_note_renames_and_refuses_to_clobber() {
        let (_dir, vault) = vault_with(&[("Old.md", "x"), ("Taken.md", "y")]);
        assert_eq!(
            vault.move_note("Old", "Archive/New").unwrap(),
            "Archive/New.md"
        );
        assert_eq!(vault.read_note("Archive/New").unwrap(), "x");
        assert!(matches!(
            vault.read_note("Old"),
            Err(VaultError::NotFound(_))
        ));
        assert!(matches!(
            vault.move_note("Archive/New", "Taken"),
            Err(VaultError::AlreadyExists(_))
        ));
        assert!(matches!(
            vault.move_note("Missing", "Elsewhere"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn search_is_case_insensitive_and_requires_all_terms() {
        let (_dir, vault) = vault_with(&[
            ("one.md", "Rust is great\nnothing here\nI like RUST and tea"),
            ("two.md", "rust only"),
            ("three.md", "tea only"),
        ]);
        let results = vault.search("rust tea", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "one.md");
        assert_eq!(
            results[0].matches,
            vec![
                SearchMatch {
                    line: 1,
                    text: "Rust is great".into()
                },
                SearchMatch {
                    line: 3,
                    text: "I like RUST and tea".into()
                },
            ]
        );
    }

    #[test]
    fn search_matches_file_names_and_respects_limit() {
        let (_dir, vault) = vault_with(&[
            ("Projects/Alpha.md", "body"),
            ("b.md", "alpha"),
            ("c.md", "alpha"),
        ]);
        let results = vault.search("alpha", 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path, "Projects/Alpha.md");
        assert!(results[0].matches.is_empty());
    }

    #[test]
    fn search_rejects_empty_query() {
        let (_dir, vault) = vault_with(&[]);
        assert!(matches!(
            vault.search("  ", 10),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn backlinks_match_by_name_or_path_case_insensitively() {
        let (_dir, vault) = vault_with(&[
            ("Folder/Target.md", "target"),
            ("a.md", "links to [[target]]"),
            ("b.md", "links to [[Folder/Target|alias]]"),
            ("c.md", "links to [[Target.md#Heading]]"),
            ("d.md", "links to [[Other]]"),
        ]);
        assert_eq!(
            vault.backlinks("Folder/Target").unwrap(),
            vec!["a.md", "b.md", "c.md"]
        );
        assert!(matches!(
            vault.backlinks("Missing"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn tags_are_counted_per_note() {
        let (_dir, vault) = vault_with(&[
            ("a.md", "#rust #idea #rust"),
            ("b.md", "---\ntags: [rust]\n---\n"),
        ]);
        let tags = vault.tags().unwrap();
        assert_eq!(tags.get("rust"), Some(&2));
        assert_eq!(tags.get("idea"), Some(&1));
    }

    #[test]
    fn notes_with_tag_matches_nested_tags_and_ignores_hash() {
        let (_dir, vault) = vault_with(&[
            ("a.md", "#project/alpha"),
            ("b.md", "#Project"),
            ("c.md", "#projects"),
        ]);
        assert_eq!(
            vault.notes_with_tag("#project").unwrap(),
            vec!["a.md", "b.md"]
        );
    }

    #[test]
    fn note_info_collects_metadata() {
        let (_dir, vault) = vault_with(&[
            ("Note.md", "---\nstatus: draft\n---\n#todo see [[Other]]"),
            ("Other.md", "back to [[Note]]"),
        ]);
        let info = vault.note_info("Note").unwrap();
        assert_eq!(info.path, "Note.md");
        assert_eq!(info.frontmatter["status"], "draft");
        assert_eq!(info.tags, vec!["todo"]);
        assert_eq!(info.links, vec!["Other"]);
        assert_eq!(info.backlinks, vec!["Other.md"]);
        assert!(info.size_bytes > 0);
    }
}
