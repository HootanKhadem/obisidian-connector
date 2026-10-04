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
curl -fsSL https://raw.githubusercontent.com/HootanKhadem/obisidian-connector/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/HootanKhadem/obisidian-connector/main/install.ps1 | iex
```

**With Cargo**

```sh
cargo install --git https://github.com/HootanKhadem/obisidian-connector
```

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
- `--read-only` removes every tool that changes the vault.

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

To publish a release, push a `v*` tag. The release workflow builds binaries for
Linux, macOS and Windows, which the install scripts then download.

## License

MIT
