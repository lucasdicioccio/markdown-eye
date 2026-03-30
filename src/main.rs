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
use sassy::{Searcher, profiles::Ascii};
use serde::Deserialize;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct Config {
    /// Preferred colour theme. `true` = dark, `false` = light.
    /// Absent → light (egui default).
    dark_mode: Option<bool>,
}

fn load_config() -> Config {
    let path = std::env::var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("markdown-eye")
                .join("config.toml")
        })
        .unwrap_or_default();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

// ── Bookmarks ─────────────────────────────────────────────────────────────────

struct BookmarkItem {
    title: String,
    target: String, // absolute file path or URL
    summary: String,
}

fn bookmark_file_path() -> PathBuf {
    std::env::var("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join(".config")
                .join("markdown-eye")
                .join("bookmarks.md")
        })
        .unwrap_or_default()
}

/// Parse a heading line like `## [Title](target)` into `(title, target)`.
fn parse_bookmark_heading(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("## [")?;
    let bracket_end = rest.find("](")?;
    let title = rest[..bracket_end].to_string();
    let after = &rest[bracket_end + 2..];
    let paren_end = after.rfind(')')?;
    let target = after[..paren_end].to_string();
    Some((title, target))
}

fn load_bookmarks(path: &PathBuf) -> Vec<BookmarkItem> {
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut items: Vec<BookmarkItem> = Vec::new();
    // (title, target, accumulated summary lines)
    let mut current: Option<(String, String, Vec<String>)> = None;
    for line in content.lines() {
        if let Some((title, target)) = parse_bookmark_heading(line) {
            if let Some((t, tg, summary_lines)) = current.take() {
                items.push(BookmarkItem {
                    title: t,
                    target: tg,
                    summary: summary_lines.join("\n").trim().to_string(),
                });
            }
            current = Some((title, target, Vec::new()));
        } else if let Some((_, _, ref mut lines)) = current {
            lines.push(line.to_string());
        }
    }
    if let Some((t, tg, summary_lines)) = current {
        items.push(BookmarkItem {
            title: t,
            target: tg,
            summary: summary_lines.join("\n").trim().to_string(),
        });
    }
    items
}

fn save_bookmarks(path: &PathBuf, items: &[BookmarkItem]) {
    let mut out = String::from("# markdown-eye bookmarks\n");
    for item in items {
        out.push('\n');
        out.push_str(&format!("## [{}]({})\n", item.title, item.target));
        if !item.summary.is_empty() {
            out.push('\n');
            out.push_str(&item.summary);
            if !item.summary.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, out);
}

/// Canonical string key for a tab: URL as-is, file path canonicalised (or raw).
fn tab_target(tab: &FileTab) -> String {
    if let Some(url) = &tab.url {
        url.clone()
    } else {
        tab.path
            .canonicalize()
            .unwrap_or_else(|_| tab.path.clone())
            .to_string_lossy()
            .into_owned()
    }
}

// ── Table extraction & copy helpers ───────────────────────────────────────────

struct TocSection {
    level: u8,       // 0 = preamble (before any heading), 1–3 = heading level
    heading: String, // heading text (empty for preamble)
    segments: Vec<Segment>,
}

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

/// Parse a line as a heading (levels 1–3 only). Returns `(level, text)`.
fn parse_heading_line(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count() as u8;
    if hashes == 0 || hashes > 3 {
        return None;
    }
    let rest = &trimmed[hashes as usize..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // `#title` without space is not a heading
    }
    Some((hashes, rest.trim().to_string()))
}

/// Split content into sections at h1–h3 heading boundaries.
/// The first section may have `level == 0` (preamble before any heading).
fn parse_toc_sections(content: &str) -> Vec<TocSection> {
    let mut sections: Vec<TocSection> = Vec::new();
    let mut current_lines: Vec<&str> = Vec::new();
    let mut current_level: u8 = 0;
    let mut current_heading = String::new();

    for line in content.lines() {
        if let Some((level, heading)) = parse_heading_line(line) {
            sections.push(TocSection {
                level: current_level,
                heading: current_heading.clone(),
                segments: parse_segments(&current_lines.join("\n")),
            });
            current_lines.clear();
            current_level = level;
            current_heading = heading;
        }
        current_lines.push(line);
    }
    sections.push(TocSection {
        level: current_level,
        heading: current_heading,
        segments: parse_segments(&current_lines.join("\n")),
    });
    sections
}

// ── Search helpers ────────────────────────────────────────────────────────────

/// Concatenate a section's heading and all segment text for searching.
fn section_text(section: &TocSection) -> String {
    let mut text = section.heading.clone();
    for seg in &section.segments {
        text.push('\n');
        match seg {
            Segment::Markdown(s) => text.push_str(s),
            Segment::Table { raw, .. } => text.push_str(raw),
        }
    }
    text
}

/// Maximum edit-distance budget based on query length.
/// Short queries use k=0 (exact only) to avoid false positives.
fn search_k(query_len: usize) -> usize {
    if query_len < 4 { 0 } else { 1 }
}

/// Return the minimum edit cost of any match of `query_lower` (already-lowercased bytes)
/// in `content` at tolerance `k`, or `None` if there are no matches.
fn best_match_cost(query_lower: &[u8], content: &str, k: usize) -> Option<usize> {
    if query_lower.is_empty() || content.is_empty() {
        return None;
    }
    let lower = content.to_lowercase();
    let mut searcher = Searcher::<Ascii>::new_fwd();
    searcher
        .search(query_lower, lower.as_bytes(), k)
        .iter()
        .map(|m| m.cost as usize)
        .min()
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
        dark_mode: bool,
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
            dark_mode,
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
    toc_sections: Vec<TocSection>,
    scroll_to: Option<usize>,
    from_stdin: bool,
    is_bookmark: bool,
    bookmark_title: Option<String>,
}

impl FileTab {
    fn load(path: PathBuf) -> Result<Self, String> {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        let toc_sections = parse_toc_sections(&content);
        Ok(Self {
            path,
            url: None,
            content,
            cache: CommonMarkCache::default(),
            toc_sections,
            scroll_to: None,
            from_stdin: false,
            is_bookmark: false,
            bookmark_title: None,
        })
    }

    fn load_url(url: String) -> Result<Self, String> {
        let content = ureq::get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .into_string()
            .map_err(|e| format!("{url}: {e}"))?;
        let toc_sections = parse_toc_sections(&content);
        Ok(Self {
            path: PathBuf::new(),
            url: Some(url),
            content,
            cache: CommonMarkCache::default(),
            toc_sections,
            scroll_to: None,
            from_stdin: false,
            is_bookmark: false,
            bookmark_title: None,
        })
    }

    fn reload(&mut self) {
        if self.url.is_some() {
            return;
        }
        if let Ok(content) = fs::read_to_string(&self.path) {
            self.toc_sections = parse_toc_sections(&content);
            self.content = content;
            self.cache = CommonMarkCache::default();
            self.scroll_to = None;
        }
    }

    fn display_name(&self) -> &str {
        if let Some(title) = &self.bookmark_title {
            return title.as_str();
        }
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
    show_toc: bool,
    tab_search: String,
    // Content search
    search_query: String,
    search_last_query: String,
    /// Per-tab: `(section_idx, best_cost)` pairs sorted by section index.
    /// cost == 0 → exact match; cost > 0 → approximate (typo-tolerant) match.
    search_hits: Vec<Vec<(usize, usize)>>,
    /// Which hit within `search_hits[active]` is the "active" (navigated-to) one.
    search_nav_idx: usize,
    /// Set when a tab reloads so search results are rebuilt next frame.
    search_stale: bool,
    bookmarks: Vec<BookmarkItem>,
    bookmark_path: PathBuf,
}

impl App {
    fn new(
        session_tabs: Vec<FileTab>,
        dark_mode: bool,
        bookmarks: Vec<BookmarkItem>,
        bookmark_path: PathBuf,
    ) -> Self {
        // Load bookmark tabs (silently skip ones that fail to load).
        let bookmark_tabs: Vec<FileTab> = bookmarks
            .iter()
            .filter_map(|item| {
                let result = if item.target.starts_with("http://")
                    || item.target.starts_with("https://")
                {
                    FileTab::load_url(item.target.clone()).ok()
                } else {
                    FileTab::load(PathBuf::from(&item.target)).ok()
                };
                result.map(|mut t| {
                    t.is_bookmark = true;
                    t.bookmark_title = Some(item.title.clone());
                    t
                })
            })
            .collect();

        let mut tabs = session_tabs;
        tabs.extend(bookmark_tabs);

        let (tx, rx) = mpsc::channel::<PathBuf>();

        let watched_paths: Vec<PathBuf> = tabs
            .iter()
            .filter(|t| t.url.is_none() && !t.from_stdin)
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

        let n = tabs.len();
        Self {
            tabs,
            active: 0,
            _watcher: watcher,
            reload_rx: rx,
            dark_mode,
            show_toc: true,
            tab_search: String::new(),
            search_query: String::new(),
            search_last_query: String::new(),
            search_hits: vec![vec![]; n],
            search_nav_idx: 0,
            search_stale: false,
            bookmarks,
            bookmark_path,
        }
    }

    /// Re-run sassy search across all tabs and populate `search_hits`.
    fn rerun_search(&mut self) {
        let query_lower: Vec<u8> = self.search_query.to_lowercase().into_bytes();
        let k = search_k(query_lower.len());
        self.search_hits = self.tabs.iter().map(|tab| {
            if query_lower.is_empty() {
                return vec![];
            }
            tab.toc_sections
                .iter()
                .enumerate()
                .filter_map(|(i, sec)| {
                    best_match_cost(&query_lower, &section_text(sec), k).map(|cost| (i, cost))
                })
                .collect()
        }).collect();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Reload changed files ──────────────────────────────────────────────
        while let Ok(changed_path) = self.reload_rx.try_recv() {
            for tab in &mut self.tabs {
                if let (Ok(a), Ok(b)) = (tab.path.canonicalize(), changed_path.canonicalize()) {
                    if a == b {
                        tab.reload();
                        self.search_stale = true;
                    }
                }
            }
            ctx.request_repaint();
        }

        // ── Re-run search when the query changes or a tab reloads ────────────
        let query_changed = self.search_query != self.search_last_query;
        if query_changed || self.search_stale {
            self.search_last_query = self.search_query.clone();
            self.search_stale = false;
            self.rerun_search();
            if query_changed {
                self.search_nav_idx = 0;
                // Auto-scroll to first match in the active tab.
                if let Some(&(sec, _)) = self.search_hits.get(self.active).and_then(|h| h.first()) {
                    self.tabs[self.active].scroll_to = Some(sec);
                }
            }
        }

        // ── Derive per-frame search display state ─────────────────────────────
        // Clone now to avoid borrow conflicts inside the closures below.
        // Each entry: (section_idx, best_cost).  cost==0 → exact, cost>0 → approx.
        let active_hits: Vec<(usize, usize)> = self
            .search_hits
            .get(self.active)
            .cloned()
            .unwrap_or_default();
        let active_hit_section: Option<usize> = if !active_hits.is_empty()
            && !self.search_query.is_empty()
        {
            Some(active_hits[self.search_nav_idx.min(active_hits.len() - 1)].0)
        } else {
            None
        };
        let matching_tab_count = self.search_hits.iter().filter(|h| !h.is_empty()).count();

        // ── Visuals ───────────────────────────────────────────────────────────
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        // Ctrl+F focuses the search box.
        let search_box_id = egui::Id::new("content_search_box");
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(search_box_id));
        }

        // ── Top panel ─────────────────────────────────────────────────────────
        let mut scroll_to_request: Option<usize> = None;
        let mut toggle_bookmark = false;
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // ── File dropdown ──────────────────────────────────────────────
                let current_name = self.tabs[self.active].display_name().to_owned();
                let btn = ui.button(format!("▾  {current_name}"));
                let popup_id = egui::Popup::default_response_id(&btn);
                {
                    let tab_search = &mut self.tab_search;
                    let tabs = &self.tabs;
                    let active = &mut self.active;
                    let search_hits = &self.search_hits;
                    let has_content_query = !self.search_query.is_empty();
                    egui::Popup::from_toggle_button_response(&btn)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .layout(egui::Layout::top_down_justified(egui::Align::LEFT))
                        .show(|ui| {
                            ui.set_min_width(320.0);
                            ui.add(
                                egui::TextEdit::singleline(tab_search)
                                    .hint_text("Search files…")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.separator();
                            let filter = tab_search.to_lowercase();
                            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                let mut shown_bookmark_section = false;
                                for i in 0..tabs.len() {
                                    let name = tabs[i].display_name().to_owned();
                                    if !filter.is_empty()
                                        && !name.to_lowercase().contains(&filter)
                                    {
                                        continue;
                                    }
                                    // Insert section header before the first visible bookmark tab.
                                    if tabs[i].is_bookmark && !shown_bookmark_section {
                                        ui.separator();
                                        ui.small("Bookmarks");
                                        shown_bookmark_section = true;
                                    }
                                    // Show exact + approx section counts next to the name.
                                    let label = if has_content_query {
                                        let hits = search_hits.get(i).map(|h| h.as_slice()).unwrap_or(&[]);
                                        let exact = hits.iter().filter(|(_, c)| *c == 0).count();
                                        let approx = hits.iter().filter(|(_, c)| *c > 0).count();
                                        match (exact, approx) {
                                            (0, 0) => name,
                                            (e, 0) => format!("{name}  ({e})"),
                                            (0, a) => format!("{name}  (~{a})"),
                                            (e, a) => format!("{name}  ({e}+~{a})"),
                                        }
                                    } else {
                                        name
                                    };
                                    if ui.selectable_value(active, i, label).clicked() {
                                        egui::Popup::close_id(ui.ctx(), popup_id);
                                        tab_search.clear();
                                    }
                                }
                            });
                        });
                }

                // ── Content search bar ─────────────────────────────────────────
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .id(search_box_id)
                        .hint_text("🔍  Search…")
                        .desired_width(200.0),
                );
                // Escape clears the query when the search box is focused.
                if ctx.memory(|m| m.has_focus(search_box_id))
                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    self.search_query.clear();
                }

                // ── Navigation + right-side controls ──────────────────────────
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Theme toggle and TOC toggle (rightmost items).
                    let icon = if self.dark_mode { "☀" } else { "🌙" };
                    if ui.button(icon).clicked() {
                        self.dark_mode = !self.dark_mode;
                    }
                    let has_toc = self.tabs[self.active]
                        .toc_sections
                        .iter()
                        .any(|s| s.level > 0);
                    if has_toc {
                        let toc_icon = if self.show_toc { "≡·" } else { "≡" };
                        ui.toggle_value(&mut self.show_toc, toc_icon)
                            .on_hover_text("Toggle table of contents");
                    }

                    // Bookmark toggle (hidden for stdin).
                    if !self.tabs[self.active].from_stdin {
                        let target = tab_target(&self.tabs[self.active]);
                        let is_bm = self.bookmarks.iter().any(|b| b.target == target);
                        let bm_icon = if is_bm { "★" } else { "☆" };
                        let bm_tip = if is_bm { "Remove bookmark" } else { "Add bookmark" };
                        if ui.button(bm_icon).on_hover_text(bm_tip).clicked() {
                            toggle_bookmark = true;
                        }
                    }

                    if !self.search_query.is_empty() {
                        if !active_hits.is_empty() {
                            if ui.small_button("▼").clicked() {
                                self.search_nav_idx =
                                    (self.search_nav_idx + 1) % active_hits.len();
                                scroll_to_request = Some(active_hits[self.search_nav_idx].0);
                            }
                            if ui.small_button("▲").clicked() {
                                self.search_nav_idx = if self.search_nav_idx == 0 {
                                    active_hits.len() - 1
                                } else {
                                    self.search_nav_idx - 1
                                };
                                scroll_to_request = Some(active_hits[self.search_nav_idx].0);
                            }
                            let nav = self.search_nav_idx.min(active_hits.len() - 1) + 1;
                            let exact = active_hits.iter().filter(|(_, c)| *c == 0).count();
                            let approx = active_hits.iter().filter(|(_, c)| *c > 0).count();
                            let counts = match (exact, approx) {
                                (e, 0) => format!("{e} exact"),
                                (0, a) => format!("~{a} approx"),
                                (e, a) => format!("{e} exact + ~{a} approx"),
                            };
                            let info = if matching_tab_count > 1 {
                                format!("{nav}/{} · {counts} · {matching_tab_count} files", active_hits.len())
                            } else {
                                format!("{nav}/{} · {counts}", active_hits.len())
                            };
                            ui.small(&info);
                        } else {
                            ui.small("no matches");
                        }
                    }
                });
            });
        });

        // Apply any navigation scroll (after the panel, before the central panel runs).
        if let Some(sec) = scroll_to_request {
            self.tabs[self.active].scroll_to = Some(sec);
        }

        // ── Bookmark toggle ───────────────────────────────────────────────────
        if toggle_bookmark {
            let target = tab_target(&self.tabs[self.active]);
            let title = self.tabs[self.active].display_name().to_owned();
            let bm_idx = self.bookmarks.iter().position(|b| b.target == target);
            let bm_path = self.bookmark_path.clone();
            if let Some(idx) = bm_idx {
                // Remove the bookmark entry.
                self.bookmarks.remove(idx);
                save_bookmarks(&bm_path, &self.bookmarks);
                // Remove the associated bookmark tab (active if it is one, else find it).
                let tab_to_remove = if self.tabs[self.active].is_bookmark {
                    Some(self.active)
                } else {
                    self.tabs.iter().position(|t| t.is_bookmark && tab_target(t) == target)
                };
                if let Some(ti) = tab_to_remove {
                    self.tabs.remove(ti);
                    self.search_hits.remove(ti);
                    if self.active > ti {
                        self.active -= 1;
                    } else if self.active >= self.tabs.len() {
                        self.active = self.tabs.len().saturating_sub(1);
                    }
                }
            } else {
                // Add the bookmark entry.
                self.bookmarks.push(BookmarkItem {
                    title: title.clone(),
                    target: target.clone(),
                    summary: String::new(),
                });
                save_bookmarks(&bm_path, &self.bookmarks);
                // Add a bookmark tab if this target isn't already one.
                let already = self.tabs.iter().any(|t| t.is_bookmark && tab_target(t) == target);
                if !already {
                    let new_tab = if target.starts_with("http://") || target.starts_with("https://") {
                        FileTab::load_url(target).ok()
                    } else {
                        FileTab::load(PathBuf::from(&target)).ok()
                    };
                    if let Some(mut t) = new_tab {
                        t.is_bookmark = true;
                        t.bookmark_title = Some(title);
                        self.tabs.push(t);
                        self.search_hits.push(vec![]);
                        self.search_stale = true;
                    }
                }
            }
        }

        // ── TOC sidebar ───────────────────────────────────────────────────────
        let has_toc = self.tabs[self.active]
            .toc_sections
            .iter()
            .any(|s| s.level > 0);

        if has_toc && self.show_toc {
            let mut clicked_section: Option<usize> = None;
            egui::SidePanel::left("toc")
                .resizable(true)
                .default_width(180.0)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.strong("Contents");
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let tab = &self.tabs[self.active];
                        let dark = ui.visuals().dark_mode;
                        for (i, section) in tab.toc_sections.iter().enumerate() {
                            if section.level == 0 {
                                continue;
                            }
                            let indent = (section.level - 1) as f32 * 14.0;
                            let is_active_hit = active_hit_section == Some(i);
                            let hit_cost = active_hits.iter().find(|(idx, _)| *idx == i).map(|(_, c)| *c);
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                let text_color = match hit_cost {
                                    Some(0) if is_active_hit => if dark {
                                        egui::Color32::from_rgb(255, 160, 50)   // bright orange
                                    } else {
                                        egui::Color32::from_rgb(200, 80, 0)
                                    },
                                    Some(0) => if dark {
                                        egui::Color32::from_rgb(200, 160, 60)   // amber
                                    } else {
                                        egui::Color32::from_rgb(150, 100, 0)
                                    },
                                    Some(_) if is_active_hit => if dark {
                                        egui::Color32::from_rgb(110, 180, 255)  // bright blue
                                    } else {
                                        egui::Color32::from_rgb(30, 100, 200)
                                    },
                                    Some(_) => if dark {
                                        egui::Color32::from_rgb(80, 140, 210)   // muted blue
                                    } else {
                                        egui::Color32::from_rgb(60, 110, 185)
                                    },
                                    None => if dark {
                                        ui.visuals().hyperlink_color
                                    } else {
                                        egui::Color32::from_rgb(70, 110, 160)
                                    },
                                };
                                let base = egui::RichText::new(&section.heading)
                                    .small()
                                    .color(text_color);
                                let rich = if is_active_hit { base.strong() } else { base };
                                let label = egui::Label::new(rich)
                                    .sense(egui::Sense::click())
                                    .truncate();
                                let response = ui
                                    .add(label)
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                if response.clicked() {
                                    clicked_section = Some(i);
                                }
                            });
                        }
                    });
                });
            if let Some(idx) = clicked_section {
                self.tabs[self.active].scroll_to = Some(idx);
            }
        }

        // ── Central panel ─────────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let tab = &mut self.tabs[self.active];
                    let scroll_to = tab.scroll_to.take();
                    let dark = ui.visuals().dark_mode;
                    for (i, section) in tab.toc_sections.iter().enumerate() {
                        if scroll_to == Some(i) {
                            ui.scroll_to_cursor(Some(egui::Align::TOP));
                        }
                        let is_active_hit = active_hit_section == Some(i);
                        let hit_cost = active_hits.iter().find(|(idx, _)| *idx == i).map(|(_, c)| *c);
                        if let Some(cost) = hit_cost {
                            let exact = cost == 0;
                            // Fill: orange tones for exact, blue tones for approx.
                            let fill = match (exact, is_active_hit, dark) {
                                (true,  true,  true)  => egui::Color32::from_rgba_unmultiplied(255, 140,   0, 30),
                                (true,  true,  false) => egui::Color32::from_rgba_unmultiplied(255, 140,   0, 25),
                                (true,  false, true)  => egui::Color32::from_rgba_unmultiplied(255, 220,   0, 12),
                                (true,  false, false) => egui::Color32::from_rgba_unmultiplied(255, 220,   0, 20),
                                (false, true,  true)  => egui::Color32::from_rgba_unmultiplied( 80, 150, 255, 30),
                                (false, true,  false) => egui::Color32::from_rgba_unmultiplied( 80, 130, 255, 22),
                                (false, false, true)  => egui::Color32::from_rgba_unmultiplied( 80, 150, 255, 12),
                                (false, false, false) => egui::Color32::from_rgba_unmultiplied( 80, 130, 255, 18),
                            };
                            let stroke_color = match (exact, is_active_hit) {
                                (true,  true)  => egui::Color32::from_rgb(220, 120,   0),
                                (true,  false) => egui::Color32::from_rgba_unmultiplied(200, 160,   0, 120),
                                (false, true)  => egui::Color32::from_rgb( 70, 130, 220),
                                (false, false) => egui::Color32::from_rgba_unmultiplied( 70, 130, 220, 110),
                            };
                            egui::Frame::new()
                                .fill(fill)
                                .stroke(egui::Stroke::new(1.5, stroke_color))
                                .inner_margin(egui::Margin::same(6))
                                .corner_radius(egui::CornerRadius::same(4))
                                .show(ui, |ui| {
                                    render_segments(ui, &mut tab.cache, &section.segments);
                                });
                        } else {
                            render_segments(ui, &mut tab.cache, &section.segments);
                        }
                    }
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

fn launch(
    tabs: Vec<FileTab>,
    portrait: bool,
    dark_mode: bool,
    bookmarks: Vec<BookmarkItem>,
    bookmark_path: PathBuf,
) {
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
        Box::new(move |_cc| Ok(Box::new(App::new(tabs, dark_mode, bookmarks, bookmark_path)))),
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
    let config = load_config();
    let dark_mode = config.dark_mode.unwrap_or(false);
    let bookmark_path = bookmark_file_path();
    let bookmarks = load_bookmarks(&bookmark_path);

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
                    Box::new(move |_cc| {
                        Ok(Box::new(FormApp::new(markdown, schema, result_clone, dark_mode)))
                    }),
                )
                .unwrap();

                let output = result.lock().unwrap().take();
                match output {
                    Some(json) => println!("{json}"),
                    None => std::process::exit(1),
                }
            } else {
                let toc_sections = parse_toc_sections(&content);
                let tab = FileTab {
                    path: PathBuf::from("/dev/stdin"),
                    url: None,
                    content,
                    cache: CommonMarkCache::default(),
                    toc_sections,
                    scroll_to: None,
                    from_stdin: true,
                    is_bookmark: false,
                    bookmark_title: None,
                };
                launch(vec![tab], portrait, dark_mode, bookmarks, bookmark_path);
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

            launch(tabs, portrait, dark_mode, bookmarks, bookmark_path);
        }
    }
}
