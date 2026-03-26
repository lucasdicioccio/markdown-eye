# markdown-eye

A fast, native Linux markdown viewer built in Rust. Opens a GPU-accelerated GUI window to render markdown — from files, URLs, or stdin — with live reload, a table of contents, cross-file fuzzy search, interactive forms, and an [agents-exe](https://github.com/lucasdicioccio/agents-exe) integration.

## Features

- **Multi-file viewer** — open any number of files as tabs, switch via a searchable dropdown
- **Table of contents** — collapsible sidebar built from `#`/`##`/`###` headings; click to jump
- **Search** — cross-file, typo-tolerant search powered by [sassy](https://crates.io/crates/sassy); distinct colours for exact vs approximate matches
- **Table extraction** — copy any markdown table as CSV or JSON with one click
- **URL support** — pass `http://`/`https://` URLs as file arguments
- **Live reload** — files update automatically when saved; search index rebuilt on reload
- **Interactive forms** — embed a `form` block to present a GUI form; results printed as JSON to stdout
- **agents-exe integration** — implements the binary tool protocol (`describe` / `run`)
- **Dark / Light theme** toggle
- **Syntax highlighting** in code blocks

## Installation

```bash
cargo install --path .
# or just build:
cargo build --release   # → ./target/release/markdown-eye
```

## Usage

### Quick start

Subcommands are optional — the binary infers the right command from its arguments:

| Invocation | Equivalent to |
|---|---|
| `markdown-eye` | `markdown-eye run` (read stdin) |
| `markdown-eye file.md` | `markdown-eye view file.md` |
| `markdown-eye a.md b.md` | `markdown-eye view a.md b.md` |

```bash
# Pipe markdown from stdin
echo "# Hello" | markdown-eye

# Open a single file
markdown-eye README.md

# Open multiple files as tabs
markdown-eye README.md CHANGELOG.md docs/guide.md
```

### `view` — Open one or more files

```bash
markdown-eye view <FILES>... [--portrait|--landscape]
```

Each argument can be a local path or an `http://`/`https://` URL. Multiple files are opened as tabs. Local files are watched for changes and reloaded automatically.

```bash
markdown-eye view notes.md --portrait
markdown-eye view README.md https://raw.githubusercontent.com/…/CHANGELOG.md
```

### `run` — Render stdin

```bash
markdown-eye run [--portrait|--landscape] [--mode view|form|echo-instructions]
```

Reads all of stdin as markdown, then opens a window with the rendered result. If the content contains a [`form` block](#interactive-forms), the form UI is shown automatically (override with `--mode`).

```bash
echo "# Hello" | markdown-eye run
cat report.md | markdown-eye run --portrait
pandoc input.docx -t markdown | markdown-eye run
```

| `--mode` value | Behaviour |
|---|---|
| *(omitted)* | Auto-detect: form UI if a `form` block is found, plain view otherwise |
| `view` | Plain markdown; `form` blocks are ignored |
| `form` | Force form UI; exits with an error if no valid `form` block is found |
| `echo-instructions` | Print the form-authoring guide to stdout and exit — **stdin is not read** |

### Window size

| Flag | Dimensions | Use case |
|---|---|---|
| `--landscape` *(default)* | 1200 × 800 | Wide documents |
| `--portrait` | 800 × 1100 | Long documents, A4-like layout |

---

## UI overview

### File selector

The **▾ filename** button in the top-left opens a dropdown listing all open files. A **"Search files…"** text box at the top of the dropdown filters by filename.

When a [content search](#search) is active, each file in the list shows a match summary:

| Suffix | Meaning |
|---|---|
| `(3)` | 3 sections with exact matches |
| `(~2)` | 2 sections with approximate (fuzzy) matches |
| `(2+~1)` | 2 exact + 1 approximate |

### Table of contents

Documents with `#`, `##`, or `###` headings get a collapsible TOC sidebar. Click any entry to scroll there. Toggle it with the **≡** button (top-right, only shown when the document has headings).

Matching sections are colour-coded when a search is active — see [Search highlighting](#highlighting).

### Table extraction

Markdown tables are parsed automatically. Two copy buttons appear above each table:

- **Copy CSV** — RFC 4180 compliant; cells with commas or quotes are quoted and escaped
- **Copy JSON** — Array of objects, one key per header column

### Dark mode

Toggle between light and dark themes with the **🌙 / ☀** button (top-right). Defaults to light.

---

## Search

A **🔍 Search…** bar is always visible in the top bar.

| Shortcut | Action |
|---|---|
| **Ctrl+F** | Focus the search bar |
| **Escape** | Clear the query (when the search bar is focused) |

### Typo tolerance

Search uses [sassy](https://crates.io/crates/sassy) for SIMD-accelerated approximate string matching. The edit-distance budget scales with query length:

| Query length | Budget (k) | Effect |
|---|---|---|
| < 4 chars | 0 | Exact matches only (avoids noise on very short queries) |
| ≥ 4 chars | 1 | One insertion, deletion, or substitution tolerated |

Matches are classified by their minimum edit cost:

- **Exact** (cost = 0) — query appears verbatim (case-insensitive)
- **Approximate** (cost ≥ 1) — matched via one edit, likely a typo

### Navigation

**▲ / ▼** buttons step through matching sections in the current file. The counter shows position and a breakdown by quality:

```
3/7 · 5 exact + ~2 approx · 4 files
```

Typing auto-scrolls to the first match. Switching files resets the counter for that file.

### Highlighting

Exact and approximate matches use distinct colour schemes throughout the UI:

| Location | Exact match | Approximate match |
|---|---|---|
| TOC entry | Amber; bright orange + bold when active | Muted blue; bright blue + bold when active |
| Content section | Yellow tint + amber border | Blue tint + blue border |
| Active section | More saturated, stronger border | Same, in blue |

---

## Interactive forms

Embed a `form` fenced code block in your markdown to present an interactive form panel. The block is stripped from the rendered content — only the surrounding markdown is shown. On **Submit** the answers are printed as compact JSON to stdout. On **Cancel** or window close, the process exits with code 1 and prints nothing.

````markdown
```form
{
  "fields": [
    {"name": "username", "type": "entry",    "label": "Username",    "default": "alice"},
    {"name": "token",    "type": "password", "label": "API token"},
    {"name": "env",      "type": "list",     "label": "Environment", "options": ["staging", "production"]},
    {"name": "confirm",  "type": "question", "label": "I have reviewed the diff"}
  ]
}
```
````

Stdout on submit:

```json
{"username":"alice","token":"s3cr3t","env":"production","confirm":"yes"}
```

### Field types

| `type` | Widget | Output | Notes |
|---|---|---|---|
| `entry` | Single-line text | string | `default` pre-fills |
| `textarea` | Multi-line text | string (newlines preserved) | `default` pre-fills |
| `datetime` | Single-line text | string (no validation) | Hint: `YYYY-MM-DD HH:MM` |
| `file-location` | Text + **Browse…** button | absolute path string | Opens native file picker |
| `password` | Masked single-line text | string | `default` pre-fills (hidden) |
| `question` | Checkbox | `"yes"` or `"no"` | `"true"` pre-checks the box |
| `list` | Dropdown | selected option string | Requires `"options": [...]`; first option default |

All fields require `name`, `type`, and `label`. `default` is optional. `options` is required for `list`.

For a full reference with examples:

```bash
markdown-eye run --mode echo-instructions
```

### Full example

````markdown
# Deploy confirmation

Please review the changes before proceeding.

```form
{
  "fields": [
    {"name": "env",       "type": "list",          "label": "Target environment",  "options": ["staging", "production"]},
    {"name": "tag",       "type": "entry",         "label": "Docker image tag",    "default": "latest"},
    {"name": "deploy_at", "type": "datetime",      "label": "Scheduled time",      "default": "2026-03-08 14:00"},
    {"name": "config",    "type": "file-location", "label": "Config file"},
    {"name": "notes",     "type": "textarea",      "label": "Release notes"},
    {"name": "token",     "type": "password",      "label": "Deploy token"},
    {"name": "confirm",   "type": "question",      "label": "I have reviewed the changes"}
  ]
}
```
````

---

## agents-exe integration

`markdown-eye` implements the [agents-exe binary tool protocol](https://github.com/lucasdicioccio/agents-exe/blob/main/docs/binary-tool.md). Drop the binary in a `tools/` directory alongside `agent.json` — agents-exe discovers and registers it automatically via `describe`.

### `describe`

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
      "description": "Markdown content to display…",
      "type": "string",
      "backing_type": "string",
      "arity": "single",
      "mode": "stdin"
    },
    {
      "name": "mode",
      "description": "Operating mode. 'view' / 'form' / 'echo-instructions'.",
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

`content` uses `mode: "stdin"` — agents-exe pipes the markdown into `markdown-eye run`. The optional `mode` argument controls rendering; set it to `echo-instructions` to retrieve the form-authoring guide without opening a window.

---

## Dependencies

| Crate | Purpose |
|---|---|
| [`eframe`](https://github.com/emilk/egui/tree/master/crates/eframe) | Native window / egui rendering backend |
| [`egui_commonmark`](https://github.com/lampsitter/egui_commonmark) | CommonMark renderer with syntax highlighting |
| [`sassy`](https://crates.io/crates/sassy) | SIMD-accelerated approximate string search |
| [`notify`](https://github.com/notify-rs/notify) | File-system watcher for live reload |
| [`rfd`](https://github.com/PolyMeilex/rfd) | Native file-picker dialog |
| [`ureq`](https://github.com/algesten/ureq) | HTTP client for URL loading |
| [`clap`](https://github.com/clap-rs/clap) | CLI argument parsing |
| [`serde` / `serde_json`](https://serde.rs) | Form schema parsing and JSON output |
