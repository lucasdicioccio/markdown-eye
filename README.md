# markdown-eye

A fast, native Linux markdown viewer built in Rust.

## Features

- Render one or more `.md` files in a native window
- **Tabbed interface** — switch between files with a click
- **Syntax highlighting** in code blocks
- **Dark / Light theme toggle** (☀ / 🌙 button)
- **Live reload** — the viewer updates automatically when a file changes on disk (`view` mode)
- **stdin support** — pipe markdown directly into the viewer (`run` command)
- **Interactive forms** — embed a `form` block in the markdown to present a GUI form; results are printed as JSON to stdout
- **agents-exe integration** — implements the binary tool protocol (`describe` / `run`)

## Usage

### Quick start (shorthand invocation)

Subcommands are optional. The binary infers the right command from its arguments:

| Invocation | Equivalent to |
|---|---|
| `markdown-eye` | `markdown-eye run` (read stdin) |
| `markdown-eye file.md` | `markdown-eye view file.md` |
| `markdown-eye a.md b.md` | `markdown-eye view a.md b.md` |

```bash
# Pipe markdown from stdin
echo "# Hello" | markdown-eye

# Open a file directly
markdown-eye README.md

# Open multiple files as tabs
markdown-eye README.md CHANGELOG.md docs/guide.md --portrait
```

### View files

```bash
markdown-eye view <FILES>... [--portrait|--landscape]
```

```
Arguments:
  <FILES>...   One or more .md files to display

Options:
      --portrait   Portrait window proportions (~800x1100)
      --landscape  Landscape window proportions (~1200x800, default)
  -h, --help       Print help
  -V, --version    Print version
```

```bash
# Portrait layout (tall, like A4 paper)
markdown-eye view notes.md --portrait
```

### View from stdin

```bash
markdown-eye run [--portrait|--landscape] [--mode view|form|echo-instructions]
```

Reads all of stdin as markdown, then opens a GUI window with the rendered result.

```bash
echo "# Hello" | markdown-eye run
cat report.md | markdown-eye run --portrait
pandoc input.docx -t markdown | markdown-eye run
```

#### `--mode`

| value | behaviour |
|---|---|
| *(omitted)* | auto-detect: shows form panel if a `form` block is present, otherwise plain markdown |
| `view` | render markdown only; `form` blocks are ignored |
| `form` | always activate the form panel; exits with an error if no `form` block is found |
| `echo-instructions` | print the form-authoring guide to stdout and exit — **stdin is not read** |

```bash
# Get the form syntax guide (useful for LLM tool calls)
markdown-eye run --mode echo-instructions

# Force plain view even when a form block is present
cat form-doc.md | markdown-eye run --mode view
```

### Interactive forms

When the markdown piped to `run` contains a fenced ` ```form ` block with a JSON schema, the viewer renders the markdown alongside a form panel. On **Submit** the user's answers are printed as a JSON object to stdout. On **Cancel** (or closing the window), the process exits with code 1.

#### Form block syntax

````markdown
```form
{
  "fields": [
    {
      "name":    "field_name",
      "type":    "entry",
      "label":   "Label shown in the UI",
      "default": "optional default value"
    }
  ]
}
```
````

The `form` block is stripped from the rendered markdown — only the surrounding content is displayed.

#### Supported field types

| type | widget | output value |
|---|---|---|
| `entry` | single-line text input | string |
| `textarea` | multi-line text input | string (newlines preserved) |
| `datetime` | single-line text input with `YYYY-MM-DD HH:MM` hint | string (no validation) |
| `file-location` | text input + native file-picker button | absolute path string |
| `password` | masked text input | string |
| `question` | checkbox | `"yes"` or `"no"` |
| `list` | dropdown (requires `"options": [...]`) | selected option string |

#### Example

````markdown
# Deploy confirmation

Please review the diff above before proceeding.

```form
{
  "fields": [
    {"name": "env",        "type": "list",          "label": "Target environment",  "options": ["staging", "production"]},
    {"name": "tag",        "type": "entry",         "label": "Docker image tag",    "default": "latest"},
    {"name": "deploy_at",  "type": "datetime",      "label": "Scheduled time",      "default": "2026-03-08 14:00"},
    {"name": "config",     "type": "file-location", "label": "Config file"},
    {"name": "notes",      "type": "textarea",      "label": "Release notes"},
    {"name": "token",      "type": "password",      "label": "Deploy token"},
    {"name": "confirm",    "type": "question",      "label": "I have reviewed the changes"}
  ]
}
```
````

Stdout on submit:

```json
{"env":"production","tag":"v1.2.3","deploy_at":"2026-03-08 14:00","config":"/etc/app/prod.toml","notes":"Bumps rate limiter","token":"***","confirm":"yes"}
```

---

## agents-exe binary tool protocol

`markdown-eye` implements the [agents-exe binary tool protocol](https://github.com/lucasdicioccio/agents-exe/blob/main/docs/binary-tool.md). Place the binary in a `tools/` directory alongside `agent.json` and agents-exe will discover and register it automatically.

### describe

When invoked with `describe` as the sole argument, the tool prints its interface description as JSON:

```bash
markdown-eye describe
```

```json
{
  "slug": "markdown-eye",
  "description": "Opens a GUI window to render markdown content. When the markdown contains a ```form JSON block defining fields (entry, password, question, list), it shows an interactive form alongside the content and prints the user's answers as a JSON object to stdout on submit.",
  "args": [
    {
      "name": "content",
      "description": "Markdown content to display. Optionally include a ```form block with a JSON object {\"fields\": [...]} to define interactive form fields. Supported field types: entry (text input), textarea (multi-line text input), datetime (text input with YYYY-MM-DD HH:MM hint), file-location (text input + native file picker), password (hidden input), question (yes/no checkbox), list (dropdown, requires \"options\" array). NOTE: ignored entirely when --mode echo-instructions is set.",
      "type": "string",
      "backing_type": "string",
      "arity": "single",
      "mode": "stdin"
    },
    {
      "name": "mode",
      "description": "Operating mode. 'view': render markdown only, form blocks are ignored. 'form': show the interactive form defined in the ```form block. 'echo-instructions': print the form-authoring guide to stdout and exit — content is NOT read from stdin and the content argument is ignored entirely.",
      "type": "string",
      "backing_type": "string",
      "arity": "optional",
      "mode": "dashdashspace"
    }
  ],
  "empty-result": {
    "tag": "AddMessage",
    "contents": "User cancelled the form or closed the window without submitting"
  }
}
```

The `content` argument uses `mode: "stdin"` — agents-exe concatenates the markdown to stdin before calling `markdown-eye run`. The optional `mode` argument controls rendering behaviour; set it to `echo-instructions` to retrieve the form-authoring guide without opening a window (content is ignored in that case).

### run

agents-exe invokes the tool as:

```bash
markdown-eye run
```

with the markdown content piped to stdin. If the content contains a `form` block, the GUI presents the form and writes the result JSON to stdout when the user submits. Otherwise it simply renders the markdown.

---

## Install

### From source

```bash
git clone <repo>
cd markdown-eye
cargo install --path .
```

### Build only

```bash
cargo build --release
# Binary at: ./target/release/markdown-eye
```

## Dependencies

| Crate | Purpose |
|---|---|
| [`eframe`](https://github.com/emilk/egui/tree/master/crates/eframe) | Native window / egui backend |
| [`egui_commonmark`](https://github.com/lampsitter/egui_commonmark) | Commonmark markdown renderer |
| [`clap`](https://github.com/clap-rs/clap) | CLI argument parsing |
| [`notify`](https://github.com/notify-rs/notify) | File-system watcher for live reload |
| [`serde` / `serde_json`](https://serde.rs) | Form schema parsing and JSON output |
