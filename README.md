# markdown-eye

A fast, native Linux markdown viewer built in Rust.

## Features

- Render one or more `.md` files in a native window
- **Tabbed interface** — switch between files with a click
- **Syntax highlighting** in code blocks
- **Dark / Light theme toggle** (☀ / 🌙 button)
- **Live reload** — the viewer updates automatically when a file changes on disk

## Usage

```bash
markdown-eye <FILES>... [OPTIONS]
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

### Examples

```bash
# Open a single file
markdown-eye README.md

# Open multiple files as tabs
markdown-eye README.md CHANGELOG.md docs/guide.md

# Portrait layout (tall, like A4 paper)
markdown-eye notes.md --portrait
```

## Install

### From source

```bash
git clone <repo>
cd mrkdwn-viewer
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
