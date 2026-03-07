# markdown-eye

A fast, native Linux markdown viewer built in Rust.

## Features

- Render one or more `.md` files in a native window
- **Tabbed interface** — switch between files with a click
- **Syntax highlighting** in code blocks
- **Dark / Light theme toggle** (☀ / 🌙 button)
- **Live reload** — the viewer updates automatically when a file changes on disk (`view` mode)
- **stdin support** — pipe markdown directly into the viewer (`run` command)
- **agents-exe integration** — implements the binary tool protocol (`describe` / `run`)

## Usage

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

#### Examples

```bash
# Open a single file
markdown-eye view README.md

# Open multiple files as tabs
markdown-eye view README.md CHANGELOG.md docs/guide.md

# Portrait layout (tall, like A4 paper)
markdown-eye view notes.md --portrait
```

### View from stdin

```bash
markdown-eye run [--portrait|--landscape]
```

Reads all of stdin as markdown, then opens a GUI window with the rendered result.

```bash
echo "# Hello" | markdown-eye run
cat report.md | markdown-eye run --portrait
pandoc input.docx -t markdown | markdown-eye run
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
  "description": "Opens a GUI window to render and display markdown content",
  "args": [
    {
      "name": "content",
      "description": "The markdown content to display",
      "type": "string",
      "backing_type": "string",
      "arity": "single",
      "mode": "stdin"
    }
  ]
}
```

The single argument `content` uses `mode: "stdin"` — agents-exe concatenates the markdown content to the process stdin before calling `markdown-eye run`.

### run

agents-exe invokes the tool as:

```bash
markdown-eye run
```

with the markdown content piped to stdin. The GUI window opens and displays the rendered result.

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
