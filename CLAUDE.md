# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                      # debug build
cargo build --release            # release build → ./target/release/markdown-eye
cargo install --path .           # install to PATH
cargo check                      # fast type-check without linking
cargo clippy                     # linter
cargo test                       # run tests (none currently)
```

Testing the binary manually:

```bash
echo "# Hello" | cargo run -- run
cargo run -- view examples/lorem.md
cargo run -- describe
cargo run -- run --mode echo-instructions
```

## Architecture

The entire application is a single file: `src/main.rs` (~654 lines). There are no modules.

### Two GUI apps

**`App`** (view mode) — multi-file tabbed markdown viewer. Owns a `Vec<FileTab>`, a live-reload `notify` watcher, and an mpsc `Receiver<PathBuf>`. On each egui frame it drains the channel and reloads any changed `FileTab`. Each `FileTab` holds its path, raw markdown string, and a `CommonMarkCache`.

**`FormApp`** (form mode) — markdown viewer with an interactive form panel. Reads a `FormSchema` (deserialized from the fenced ` ```form ` JSON block), builds a parallel `Vec<FieldState>` to track user input, and on Submit serializes the states back to JSON and writes to a shared `Arc<Mutex<Option<String>>>` before closing the window. The main thread then reads that value and prints it to stdout (exit code 1 on Cancel/close).

### Flow

```
main() → parse CLI (with friendly fallbacks) → Commands::Run / View / Describe
  Run  → extract_form_schema() → FormApp or App (stdin as FileTab)
  View → FileTab::load() per file → App
```

`extract_form_schema` is a simple line-scanner (no regex) that finds the first ` ```form ` … ` ``` ` fenced block, parses the inner JSON, and returns `(Some(schema), markdown_without_block)`.

### CLI fallbacks (main)

Before clap parsing, `main` rewrites `argv`: no args → inserts `"run"`, unknown first arg → inserts `"view"`. This lets the binary be called as `markdown-eye file.md` without an explicit subcommand.

### agents-exe protocol

`describe` prints a hardcoded JSON descriptor. `run` is the normal invocation. The `content` arg uses `mode: "stdin"` so agents-exe pipes markdown to stdin; the optional `--mode` flag controls rendering.
