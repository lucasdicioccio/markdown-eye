use std::{
    fs,
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
};

use clap::{Parser, Subcommand};
use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;

// ── Table extraction & copy helpers ───────────────────────────────────────────

enum Segment {
    Markdown(String),
    Table {
        raw: String,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn parse_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(|s| s.trim().to_string()).collect()
}

fn parse_segments(content: &str) -> Vec<Segment> {
    let lines: Vec<&str> = content.lines().collect();
    let mut segments: Vec<Segment> = Vec::new();
    let mut md_lines: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let is_row = line.trim().starts_with('|');
        let next_is_sep = i + 1 < lines.len() && is_table_separator(lines[i + 1]);

        if is_row && next_is_sep {
            if !md_lines.is_empty() {
                segments.push(Segment::Markdown(md_lines.join("\n")));
                md_lines.clear();
            }

            let headers = parse_table_row(lines[i]);
            let mut raw_lines = vec![lines[i], lines[i + 1]];
            let mut rows: Vec<Vec<String>> = Vec::new();
            i += 2;

            while i < lines.len() {
                let l = lines[i];
                if l.trim().starts_with('|') && !is_table_separator(l) {
                    rows.push(parse_table_row(l));
                    raw_lines.push(l);
                    i += 1;
                } else {
                    break;
                }
            }

            segments.push(Segment::Table {
                raw: raw_lines.join("\n"),
                headers,
                rows,
            });
        } else {
            md_lines.push(line);
            i += 1;
        }
    }

    if !md_lines.is_empty() {
        segments.push(Segment::Markdown(md_lines.join("\n")));
    }

    segments
}

fn table_to_csv(headers: &[String], rows: &[Vec<String>]) -> String {
    fn escape(s: &str) -> String {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
    let mut out = headers.iter().map(|h| escape(h)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for row in rows {
        out.push_str(&row.iter().map(|c| escape(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

fn table_to_json(headers: &[String], rows: &[Vec<String>]) -> String {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for (j, header) in headers.iter().enumerate() {
                let val = row.get(j).cloned().unwrap_or_default();
                map.insert(header.clone(), serde_json::Value::String(val));
            }
            serde_json::Value::Object(map)
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(arr)).unwrap()
}

fn render_segments(ui: &mut egui::Ui, cache: &mut CommonMarkCache, segments: &[Segment]) {
    for segment in segments {
        match segment {
            Segment::Markdown(text) => {
                CommonMarkViewer::new().show(ui, cache, text);
            }
            Segment::Table { raw, headers, rows } => {
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.small_button("Copy JSON").clicked() {
                                ui.ctx().copy_text(table_to_json(headers, rows));
                            }
                            if ui.small_button("Copy CSV").clicked() {
                                ui.ctx().copy_text(table_to_csv(headers, rows));
                            }
                        },
                    );
                });
                CommonMarkViewer::new().show(ui, cache, raw);
            }
        }
    }
}

// ── Form schema ───────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone)]
struct FormField {
    name: String,
    #[serde(rename = "type")]
    field_type: String,
    label: String,
    #[serde(default)]
    default: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Deserialize)]
struct FormSchema {
    fields: Vec<FormField>,
}

enum FieldState {
    Text(String),
    Bool(bool),
    Choice(usize),
}

/// Scan `content` for a fenced ` ```form ` block containing JSON.
/// Returns `(Some(schema), markdown_without_block)` on success,
/// or `(None, content)` if no valid form block is found.
fn extract_form_schema(content: &str) -> (Option<FormSchema>, String) {
    let lines: Vec<&str> = content.lines().collect();
    let mut form_start: Option<usize> = None;
    let mut form_end: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "```form" {
            form_start = Some(i);
        } else if form_start.is_some() && line.trim() == "```" {
            form_end = Some(i);
            break;
        }
    }

    if let (Some(start), Some(end)) = (form_start, form_end) {
        let json_str = lines[start + 1..end].join("\n");
        if let Ok(schema) = serde_json::from_str::<FormSchema>(&json_str) {
            let mut remaining: Vec<&str> = Vec::new();
            remaining.extend_from_slice(&lines[..start]);
            remaining.extend_from_slice(&lines[end + 1..]);
            let cleaned = remaining.join("\n").trim().to_string();
            return (Some(schema), cleaned);
        }
    }

    (None, content.to_string())
}

// ── Form app ──────────────────────────────────────────────────────────────────

struct FormApp {
    segments: Vec<Segment>,
    cache: CommonMarkCache,
    fields: Vec<FormField>,
    states: Vec<FieldState>,
    dark_mode: bool,
    result: Arc<Mutex<Option<String>>>,
}

impl FormApp {
    fn new(
        markdown_content: String,
        schema: FormSchema,
        result: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let states = schema
            .fields
            .iter()
            .map(|f| match f.field_type.as_str() {
                "question" => FieldState::Bool(f.default == "true"),
                "list" => FieldState::Choice(0),
                _ => FieldState::Text(f.default.clone()),
            })
            .collect();

        let segments = parse_segments(&markdown_content);
        Self {
            segments,
            cache: CommonMarkCache::default(),
            fields: schema.fields,
            states,
            dark_mode: false,
            result,
        }
    }

    fn collect_result(&self) -> String {
        let mut map = serde_json::Map::new();
        for (field, state) in self.fields.iter().zip(self.states.iter()) {
            let value = match state {
                FieldState::Text(s) => serde_json::Value::String(s.clone()),
                FieldState::Bool(b) => serde_json::Value::String(
                    if *b { "yes" } else { "no" }.to_string(),
                ),
                FieldState::Choice(idx) => {
                    let opt = field.options.get(*idx).cloned().unwrap_or_default();
                    serde_json::Value::String(opt)
                }
            };
            map.insert(field.name.clone(), value);
        }
        serde_json::to_string(&serde_json::Value::Object(map)).unwrap()
    }
}

impl eframe::App for FormApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Form");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icon = if self.dark_mode { "☀" } else { "🌙" };
                    if ui.button(icon).clicked() {
                        self.dark_mode = !self.dark_mode;
                    }
                });
            });
        });

        egui::TopBottomPanel::bottom("form_panel")
            .resizable(true)
            .min_height(150.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);

                    for i in 0..self.fields.len() {
                        // Clone the parts we need to avoid simultaneous borrows
                        // while mutably accessing self.states[i].
                        let field_type = self.fields[i].field_type.clone();
                        let label = self.fields[i].label.clone();
                        let name = self.fields[i].name.clone();
                        let options = self.fields[i].options.clone();

                        ui.label(&label);

                        match field_type.as_str() {
                            "entry" => {
                                if let FieldState::Text(ref mut s) = self.states[i] {
                                    ui.text_edit_singleline(s);
                                }
                            }
                            "textarea" => {
                                if let FieldState::Text(ref mut s) = self.states[i] {
                                    ui.add(
                                        egui::TextEdit::multiline(s)
                                            .desired_rows(4)
                                            .desired_width(f32::INFINITY),
                                    );
                                }
                            }
                            "datetime" => {
                                if let FieldState::Text(ref mut s) = self.states[i] {
                                    ui.add(
                                        egui::TextEdit::singleline(s)
                                            .hint_text("YYYY-MM-DD HH:MM"),
                                    );
                                }
                            }
                            "file-location" => {
                                if let FieldState::Text(ref mut s) = self.states[i] {
                                    ui.horizontal(|ui| {
                                        ui.text_edit_singleline(s);
                                        if ui.button("Browse…").clicked() {
                                            if let Some(path) =
                                                rfd::FileDialog::new().pick_file()
                                            {
                                                *s = path.to_string_lossy().into_owned();
                                            }
                                        }
                                    });
                                }
                            }
                            "password" => {
                                if let FieldState::Text(ref mut s) = self.states[i] {
                                    ui.add(egui::TextEdit::singleline(s).password(true));
                                }
                            }
                            "question" => {
                                if let FieldState::Bool(ref mut b) = self.states[i] {
                                    ui.checkbox(b, "");
                                }
                            }
                            "list" => {
                                if let FieldState::Choice(ref mut idx) = self.states[i] {
                                    let selected = options
                                        .get(*idx)
                                        .map(|s| s.as_str())
                                        .unwrap_or("");
                                    egui::ComboBox::from_id_salt(&name)
                                        .selected_text(selected)
                                        .show_ui(ui, |ui| {
                                            for (opt_i, opt) in options.iter().enumerate() {
                                                ui.selectable_value(idx, opt_i, opt);
                                            }
                                        });
                                }
                            }
                            other => {
                                ui.label(format!("(unsupported field type: {other})"));
                            }
                        }

                        ui.add_space(6.0);
                    }

                    ui.separator();
                    ui.add_space(4.0);

                    ui.horizontal(|ui| {
                        if ui.button("Submit").clicked() {
                            let json = self.collect_result();
                            *self.result.lock().unwrap() = Some(json);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Cancel").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.add_space(4.0);
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    render_segments(ui, &mut self.cache, &self.segments);
                });
        });
    }
}

// ── Markdown viewer app ───────────────────────────────────────────────────────

struct FileTab {
    path: PathBuf,
    url: Option<String>,
    content: String,
    cache: CommonMarkCache,
    segments: Vec<Segment>,
}

impl FileTab {
    fn load(path: PathBuf) -> Result<Self, String> {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        let segments = parse_segments(&content);
        Ok(Self {
            path,
            url: None,
            content,
            cache: CommonMarkCache::default(),
            segments,
        })
    }

    fn load_url(url: String) -> Result<Self, String> {
        let content = ureq::get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .into_string()
            .map_err(|e| format!("{url}: {e}"))?;
        let segments = parse_segments(&content);
        Ok(Self {
            path: PathBuf::new(),
            url: Some(url),
            content,
            cache: CommonMarkCache::default(),
            segments,
        })
    }

    fn reload(&mut self) {
        if self.url.is_some() {
            return;
        }
        if let Ok(content) = fs::read_to_string(&self.path) {
            self.segments = parse_segments(&content);
            self.content = content;
            self.cache = CommonMarkCache::default();
        }
    }

    fn display_name(&self) -> &str {
        if let Some(url) = &self.url {
            url.trim_end_matches('/')
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(url.as_str())
        } else {
            self.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
        }
    }
}

struct App {
    tabs: Vec<FileTab>,
    active: usize,
    _watcher: Option<RecommendedWatcher>,
    reload_rx: Receiver<PathBuf>,
    dark_mode: bool,
}

impl App {
    fn new(tabs: Vec<FileTab>, dark_mode: bool) -> Self {
        let (tx, rx) = mpsc::channel::<PathBuf>();

        let watched_paths: Vec<PathBuf> = tabs
            .iter()
            .filter(|t| t.url.is_none())
            .map(|t| t.path.clone())
            .collect();

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                if matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_)
                ) {
                    for p in event.paths {
                        let _ = tx.send(p);
                    }
                }
            }
        })
        .and_then(|mut w| {
            for path in &watched_paths {
                if let Ok(abs) = path.canonicalize() {
                    let _ = w.watch(&abs, RecursiveMode::NonRecursive);
                }
            }
            Ok(w)
        })
        .ok();

        Self {
            tabs,
            active: 0,
            _watcher: watcher,
            reload_rx: rx,
            dark_mode,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(changed_path) = self.reload_rx.try_recv() {
            for tab in &mut self.tabs {
                if let (Ok(a), Ok(b)) = (tab.path.canonicalize(), changed_path.canonicalize()) {
                    if a == b {
                        tab.reload();
                    }
                }
            }
            ctx.request_repaint();
        }

        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for i in 0..self.tabs.len() {
                    let name = self.tabs[i].display_name().to_owned();
                    let selected = i == self.active;
                    if ui.selectable_label(selected, &name).clicked() {
                        self.active = i;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icon = if self.dark_mode { "☀" } else { "🌙" };
                    if ui.button(icon).clicked() {
                        self.dark_mode = !self.dark_mode;
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let tab = &mut self.tabs[self.active];
                    render_segments(ui, &mut tab.cache, &tab.segments);
                });
        });
    }
}

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Operating mode for the `run` command.
#[derive(clap::ValueEnum, Clone)]
enum RunMode {
    /// Render stdin as plain markdown; form blocks are ignored.
    View,
    /// Parse and display the interactive form embedded in the markdown.
    Form,
    /// Print the form-authoring guide to stdout and exit; stdin is not read.
    EchoInstructions,
}

#[derive(Parser)]
#[command(name = "markdown-eye", version, about = "A markdown file viewer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print tool description as JSON (agents-exe binary tool protocol)
    Describe,
    /// Display markdown content read from stdin.
    ///
    /// Default behaviour auto-detects a ```form block and switches to form
    /// mode when one is present.  Use --mode to override.
    Run {
        /// Portrait window proportions (~800x1100)
        #[arg(long, conflicts_with = "landscape")]
        portrait: bool,
        /// Landscape window proportions (~1200x800, default)
        #[arg(long, conflicts_with = "portrait")]
        landscape: bool,
        /// Operating mode (default: auto-detect)
        #[arg(long, value_enum)]
        mode: Option<RunMode>,
    },
    /// Display one or more markdown files
    View {
        /// One or more markdown files to display
        #[arg(required = true, value_name = "FILES")]
        files: Vec<PathBuf>,
        /// Portrait window proportions (~800x1100)
        #[arg(long, conflicts_with = "landscape")]
        portrait: bool,
        /// Landscape window proportions (~1200x800, default)
        #[arg(long, conflicts_with = "portrait")]
        landscape: bool,
    },
}

fn window_dims(portrait: bool) -> (f32, f32) {
    if portrait { (800.0, 1100.0) } else { (1200.0, 800.0) }
}

fn launch(tabs: Vec<FileTab>, portrait: bool) {
    let (width, height) = window_dims(portrait);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("markdown-eye")
            .with_inner_size([width, height]),
        ..Default::default()
    };
    eframe::run_native(
        "markdown-eye",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(tabs, false)))),
    )
    .unwrap();
}

fn print_form_instructions() {
    print!(
        r#"markdown-eye — form block authoring guide
==========================================

Embed a ```form fenced code block anywhere in your markdown. It is stripped
from the rendered content; only the surrounding markdown is displayed.

    ```form
    {{
      "fields": [ ... ]
    }}
    ```

Each element of "fields" is a JSON object with these properties:

  name     (string,  required) — key used in the output JSON
  type     (string,  required) — widget type; see below
  label    (string,  required) — text shown next to the widget
  default  (string,  optional) — pre-filled value
  options  (array,   required for "list") — list of selectable strings

Supported field types:

  entry         Single-line text input.
                default: any string.
                output:  string.

  textarea      Multi-line text input.
                default: any string (may include newlines).
                output:  string (newlines preserved).

  datetime      Single-line text input with a "YYYY-MM-DD HH:MM" hint.
                default: any string conforming to the format.
                output:  string (the typed value, no validation).

  file-location Single-line text input with a "Browse…" button that opens
                a native file-picker dialog.
                default: any string (pre-filled path).
                output:  absolute path string.

  password      Masked single-line text input.
                default: any string.
                output:  string.

  question      Checkbox (yes / no).
                default: "true" pre-checks the box; any other value leaves it unchecked.
                output:  "yes" or "no".

  list          Dropdown selector.
                options: required — JSON array of strings, e.g. ["a", "b", "c"].
                default: ignored (first option is selected by default).
                output:  the selected option string.

Output
------
On Submit the tool prints a compact JSON object to stdout, one key per field:

    {{"username":"alice","confirmed":"yes","env":"production"}}

On Cancel or window close the process exits with code 1.

Full example
------------

    ```form
    {{
      "fields": [
        {{"name": "username",  "type": "entry",    "label": "Username",          "default": "alice"}},
        {{"name": "token",     "type": "password", "label": "API token"}},
        {{"name": "env",       "type": "list",     "label": "Target environment","options": ["staging","production"]}},
        {{"name": "confirmed", "type": "question", "label": "I have reviewed the diff"}}
      ]
    }}
    ```
"#
    );
}

fn main() {
    // Friendly argument fallbacks:
    //   no args          → behave as `run`  (read markdown from stdin)
    //   unknown first arg → behave as `view` (treat all args as file paths)
    let raw: Vec<String> = std::env::args().collect();
    let effective: Vec<String> = if raw.len() == 1 {
        vec![raw[0].clone(), "run".to_string()]
    } else {
        let is_known = matches!(
            raw[1].as_str(),
            "describe" | "run" | "view" | "help" | "--help" | "-h" | "--version" | "-V"
        );
        if is_known {
            raw
        } else {
            let mut v = vec![raw[0].clone(), "view".to_string()];
            v.extend_from_slice(&raw[1..]);
            v
        }
    };

    let cli = Cli::parse_from(effective);

    match cli.command {
        Commands::Describe => {
            println!(
                r#"{{
  "slug": "markdown-eye",
  "description": "Opens a GUI window to render markdown content. When the markdown contains a ```form JSON block defining fields (entry, password, question, list), it shows an interactive form alongside the content and prints the user's answers as a JSON object to stdout on submit.",
  "args": [
    {{
      "name": "content",
      "description": "Markdown content to display. Optionally include a ```form block with a JSON object {{\"fields\": [...]}} to define interactive form fields. Supported field types: entry (text input), textarea (multi-line text input), datetime (text input with YYYY-MM-DD HH:MM hint), file-location (text input + native file picker), password (hidden input), question (yes/no checkbox), list (dropdown, requires \"options\" array). NOTE: ignored entirely when --mode echo-instructions is set.",
      "type": "string",
      "backing_type": "string",
      "arity": "single",
      "mode": "stdin"
    }},
    {{
      "name": "mode",
      "description": "Operating mode. 'view': render markdown only, form blocks are ignored. 'form': show the interactive form defined in the ```form block. 'echo-instructions': print the form-authoring guide to stdout and exit — content is NOT read from stdin and the content argument is ignored entirely.",
      "type": "string",
      "backing_type": "string",
      "arity": "optional",
      "mode": "dashdashspace"
    }}
  ],
  "empty-result": {{
    "tag": "AddMessage",
    "contents": "User cancelled the form or closed the window without submitting"
  }}
}}"#
            );
        }

        Commands::Run { portrait, mode, .. } => {
            // echo-instructions: no stdin needed, just print the guide and exit.
            if matches!(mode, Some(RunMode::EchoInstructions)) {
                print_form_instructions();
                return;
            }

            let content = fs::read_to_string("/dev/stdin").unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });

            // Determine whether to show a form based on explicit mode or auto-detection.
            let use_form = match mode {
                Some(RunMode::Form) => true,
                Some(RunMode::View) => false,
                _ => {
                    // Auto-detect: use form mode only if a valid form block is present.
                    let (schema, _) = extract_form_schema(&content);
                    schema.is_some()
                }
            };

            if use_form {
                let (schema, markdown) = extract_form_schema(&content);
                let schema = schema.unwrap_or_else(|| {
                    eprintln!("error: --mode form requested but no valid ```form block found in input");
                    std::process::exit(1);
                });

                let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
                let result_clone = result.clone();

                let (width, height) = window_dims(portrait);
                let options = eframe::NativeOptions {
                    viewport: egui::ViewportBuilder::default()
                        .with_title("markdown-eye")
                        .with_inner_size([width, height]),
                    ..Default::default()
                };
                eframe::run_native(
                    "markdown-eye",
                    options,
                    Box::new(|_cc| {
                        Ok(Box::new(FormApp::new(markdown, schema, result_clone)))
                    }),
                )
                .unwrap();

                let output = result.lock().unwrap().take();
                match output {
                    Some(json) => println!("{json}"),
                    None => std::process::exit(1),
                }
            } else {
                let segments = parse_segments(&content);
                let tab = FileTab {
                    path: PathBuf::from("/dev/stdin"),
                    url: None,
                    content,
                    cache: CommonMarkCache::default(),
                    segments,
                };
                launch(vec![tab], portrait);
            }
        }

        Commands::View { files, portrait, .. } => {
            let mut errors = Vec::new();
            for path in &files {
                let s = path.to_string_lossy();
                if !s.starts_with("http://") && !s.starts_with("https://") && !path.exists() {
                    errors.push(format!("File not found: {}", path.display()));
                }
            }
            if !errors.is_empty() {
                for e in errors {
                    eprintln!("error: {e}");
                }
                std::process::exit(1);
            }

            let tabs: Vec<FileTab> = files
                .into_iter()
                .filter_map(|p| {
                    let s = p.to_string_lossy();
                    if s.starts_with("http://") || s.starts_with("https://") {
                        match FileTab::load_url(s.into_owned()) {
                            Ok(t) => Some(t),
                            Err(e) => {
                                eprintln!("error: {e}");
                                None
                            }
                        }
                    } else {
                        match FileTab::load(p) {
                            Ok(t) => Some(t),
                            Err(e) => {
                                eprintln!("error: {e}");
                                None
                            }
                        }
                    }
                })
                .collect();

            if tabs.is_empty() {
                eprintln!("error: no files to display");
                std::process::exit(1);
            }

            launch(tabs, portrait);
        }
    }
}
