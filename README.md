# obsidian-connector

Give any AI agent or model access to your Obsidian vault.

`obsidian-connector` is a single small binary. It speaks the
[Model Context Protocol](https://modelcontextprotocol.io) (MCP), so it works with
Claude Desktop, Claude Code, Cursor, VS Code, Windsurf and any other MCP client.
It also exposes the same tools as a JSON command line and as OpenAI/Anthropic
function-calling schemas, for agents and models that don't speak MCP.

It reads and writes the markdown files in your vault directly. Obsidian doesn't
need to be running, and you don't need to install a plugin.

## Install

**macOS / Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/HootanKhadem/obsidain-connector/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/HootanKhadem/obsidain-connector/main/install.ps1 | iex
```

**With Cargo**

```sh
cargo install --git https://github.com/HootanKhadem/obsidain-connector
```

### Update

Run the same install command again. It replaces the binary in place, and the
config that `setup` wrote points at that same file, so you don't need to run
`setup` again. Restart your agent (Claude Desktop, Claude Code…) afterwards so
it starts the new version. On Windows the update works even while your agent is
running the old version.

Check which version you have with `obsidian-connector --version`.

**Updating to multiple-vault support (0.3.0 and later)**

1. Run the install command for your platform again, as above.
2. Your existing config keeps working and still serves one vault. To give the
   agent several vaults, run `setup` again and pass `--vault` once per vault
   (or `--all-vaults`). See [Use several vaults at once](#use-several-vaults-at-once).
   `setup` replaces the old `obsidian` entry and saves the previous config as a `.bak`.
   For Claude Code, run `claude mcp remove obsidian` first, because `claude mcp add`
   won't overwrite an existing server.
3. Restart your agent.

Nothing changes for single-vault setups: tools take the same arguments as before.

## Connect it to your agent

A single command registers the connector with your client. `--vault` takes the
vault's name as Obsidian shows it, or a path to the vault folder. If Obsidian
knows only one vault, or has one open, you can leave `--vault` out.

```sh
obsidian-connector setup claude-desktop --vault "My Vault"
obsidian-connector setup claude-code    --vault "My Vault"
obsidian-connector setup cursor         --vault ~/Documents/MyVault
obsidian-connector setup vscode         --vault "My Vault"
obsidian-connector setup windsurf       --vault "My Vault"
```

`setup` adds the server to the client's existing config. It keeps your other
servers and saves the old file as a `.bak` next to it. Restart the client
afterwards.

- Add `--read-only` to give the agent read access only.
- Add `--print` to show the config instead of writing it.

To find the names of your vaults, run `obsidian-connector vaults`.

### Use several vaults at once

Pass `--vault` more than once, or `--all-vaults` to use every vault Obsidian knows
about. One connector then serves all of them, and the agent picks a vault on each
call, so it can move between vaults in a single conversation without restarting.

```sh
obsidian-connector setup claude-desktop --vault "Work" --vault "Personal" --vault ~/Documents/Research
obsidian-connector setup claude-code --all-vaults
```

With more than one vault:

- A `list_vaults` tool returns each vault's name and path.
- Every other tool takes a required `vault` argument: the vault's name (the name
  Obsidian shows, or the folder name for a path), matched case-insensitively.
  The agent sees the names in the server instructions and can switch vaults by
  changing `vault` from one call to the next.
- Paths, links, tags and backlinks stay inside the chosen vault. To copy a note
  between vaults, the agent reads it from one and creates it in the other.
- `vault` is required, so a write never lands in the wrong vault by default.
- `--read-only` applies to all the vaults.
- Two vaults can't share a name. If two folders have the same name, give one of
  them through its Obsidian name, or rename the folder.

Example prompts: "Compare my project plan in Work with the one in Research",
or "Move the meeting notes from Personal into Work". The tool calls look like:

```json
{"name": "search_notes", "arguments": {"vault": "Work", "query": "roadmap"}}
{"name": "create_note", "arguments": {"vault": "Personal", "path": "Ideas/Roadmap", "content": "…"}}
```

The same works without MCP:

```sh
obsidian-connector call list_vaults --vault Work --vault Personal
obsidian-connector call read_note '{"vault": "Personal", "path": "Inbox"}' --vault Work --vault Personal
obsidian-connector tools --format openai --multi-vault   # schemas that include the vault argument
```

The generated config for several vaults looks like this:

```json
{
  "mcpServers": {
    "obsidian": {
      "command": "/path/to/obsidian-connector",
      "args": ["serve", "--vault", "/path/to/Work", "--vault", "/path/to/Personal"]
    }
  }
}
```

**Any other MCP client:** run `obsidian-connector setup generic --vault "My Vault"`
and paste its output into the client's config. The output looks like this:

```json
{
  "mcpServers": {
    "obsidian": {
      "command": "/path/to/obsidian-connector",
      "args": ["serve", "--vault", "/path/to/vault"]
    }
  }
}
```

## Use it without MCP

Any agent that can run shell commands can call the tools directly. Each call
prints the result to stdout. Errors go to stderr and give a non-zero exit code.

```sh
export OBSIDIAN_VAULT=~/Documents/MyVault
obsidian-connector call search_notes '{"query": "project plan"}'
obsidian-connector call read_note '{"path": "Projects/Plan"}'
echo '{"path": "Inbox", "content": "- [ ] call Sam"}' | obsidian-connector call append_to_note -
```

For function-calling APIs, export the tool schemas, give them to the model, and
run whatever tool calls the model makes with `obsidian-connector call`:

```sh
obsidian-connector tools --format openai     # OpenAI / OpenAI-compatible APIs (Ollama, vLLM, LiteLLM…)
obsidian-connector tools --format anthropic  # Anthropic Messages API
obsidian-connector tools --format mcp
```

```python
import json, subprocess

tools = json.loads(subprocess.check_output(["obsidian-connector", "tools", "--format", "openai"]))

def run_tool(name: str, arguments: dict) -> str:
    result = subprocess.run(
        ["obsidian-connector", "call", name, json.dumps(arguments)],
        capture_output=True, text=True,
    )
    return result.stdout if result.returncode == 0 else f"Error: {result.stderr}"
```

## Tools

| Tool | What it does |
| --- | --- |
| `list_vaults` | Lists the available vaults. Only present when several vaults are served |
| `list_notes` | Lists notes, optionally in one folder |
| `read_note` | Reads a note's markdown, including frontmatter. Set `heading` to read one section only |
| `create_note` | Creates a note and any missing folders. Set `overwrite` to replace an existing note |
| `append_to_note` | Appends to a note on a new line, creating the note if needed |
| `edit_note` | Replaces exact text in a note. The text must match once unless `replace_all` is set |
| `insert_into_note` | Inserts at the start or end of a note, or of a heading's section |
| `replace_section` | Replaces the body of one heading's section, keeping the heading |
| `update_frontmatter` | Sets frontmatter properties, or removes them with `null` |
| `delete_note` | Deletes a note |
| `move_note` | Moves or renames a note |
| `search_notes` | Case-insensitive search across note content and file names, returning matching lines |
| `get_backlinks` | Lists the notes that link to a note with `[[wikilinks]]` or `![[embeds]]` |
| `list_tags` | Lists every tag in the vault with the number of notes using it |
| `find_notes_by_tag` | Lists the notes with a tag; `project` also matches `project/alpha` |
| `get_note_info` | Returns a note's frontmatter, tags, outgoing links, backlinks and size |

With several vaults, every tool also takes a `vault` argument (see above).
Paths are relative to the vault. The `.md` extension is optional, so
`Projects/Plan` and `Projects/Plan.md` refer to the same note.

### Editing part of a note

The edit tools change only the part of a note they target, so an agent never has
to send the whole note back. Use `edit_note` for a precise change, `insert_into_note`
to add a line under a heading, `replace_section` to rewrite one section, and
`update_frontmatter` for properties.

Headings match case-insensitively. When a note repeats a heading, add its level
(`## Notes`) or its parent (`2024-01-01 > Notes`) to pick one.

```sh
obsidian-connector call insert_into_note '{"path": "Daily/Today", "heading": "Tasks", "content": "- [ ] call Sam"}'
obsidian-connector call edit_note '{"path": "Daily/Today", "old_text": "- [ ] call Sam", "new_text": "- [x] call Sam"}'
obsidian-connector call update_frontmatter '{"path": "Daily/Today", "properties": {"status": "done"}}'
```

## Safety

- Every path stays inside the vault. The connector rejects `..`, absolute paths,
  and symlinks that lead out of the vault.
- Hidden folders such as `.obsidian`, `.git` and `.trash` can't be read or written.
- `create_note` never overwrites a note unless the agent sets `overwrite`.
  `move_note` never replaces an existing note.
- `--read-only` removes every tool that changes the vault, in every vault.
- With several vaults, each call must name its vault, and nothing crosses between them.

## Development

Every module was written test-first, and its unit tests sit next to the code.
`tests/cli.rs` runs the compiled binary end to end, including a full MCP session.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo llvm-cov --summary-only --fail-under-lines 95   # coverage gate, enforced in CI
cargo mutants                                          # mutation testing, advisory in CI
```

Install the two tools with `cargo install cargo-llvm-cov cargo-mutants --locked`
and `rustup component add llvm-tools-preview`.

| Module | Responsibility |
| --- | --- |
| `markdown` | Frontmatter, tag and wikilink parsing |
| `edit` | Text, heading-section and frontmatter edits |
| `vault` | Sandboxed note access, search, backlinks and tags |
| `tools` | Tool definitions, argument validation, schema export |
| `mcp` | MCP JSON-RPC server over stdio |
| `discovery` | Finds the vaults registered in Obsidian |
| `setup` | Writes MCP client configuration |

### Releases

Releases are automatic. Commit messages follow
[Conventional Commits](https://www.conventionalcommits.org), and CI rejects pull
requests whose commits don't. When CI passes on `main`, the auto-release workflow
looks at the commits since the last tag:

| Commit | Release |
| --- | --- |
| `fix: …`, `perf: …` | patch, e.g. 0.2.0 → 0.2.1 |
| `feat: …` | minor, e.g. 0.2.1 → 0.3.0 |
| `feat!: …` or a `BREAKING CHANGE:` footer | major (minor while the version is 0.x) |
| `docs:`, `chore:`, `ci:`, `test:`, `refactor:`, `style:`, `build:` | no release |

It bumps the version in `Cargo.toml`, tags the commit, builds binaries for
Linux, macOS and Windows, and publishes a GitHub release with notes grouped by
type. The install scripts always download the latest release.

If you protect `main` so that the workflow's token can't push to it, add a
`RELEASE_TOKEN` repository secret holding a token that can. Pushing a `v*` tag
by hand still publishes a release too.

## License

MIT
