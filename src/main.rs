use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

use clap::Parser;
use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Parser)]
#[command(name = "markdown-eye", version, about = "A markdown file viewer")]
struct Cli {
    /// One or more markdown files to display
    #[arg(required = true, value_name = "FILES")]
    files: Vec<PathBuf>,

    /// Portrait window proportions (~800x1100)
    #[arg(long, conflicts_with = "landscape")]
    portrait: bool,

    /// Landscape window proportions (~1200x800, default)
    #[arg(long, conflicts_with = "portrait")]
    landscape: bool,
}

struct FileTab {
    path: PathBuf,
    content: String,
    cache: CommonMarkCache,
}

impl FileTab {
    fn load(path: PathBuf) -> Result<Self, String> {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        Ok(Self {
            path,
            content,
            cache: CommonMarkCache::default(),
        })
    }

    fn reload(&mut self) {
        if let Ok(content) = fs::read_to_string(&self.path) {
            self.content = content;
            self.cache = CommonMarkCache::default();
        }
    }

    fn display_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
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

        let watched_paths: Vec<PathBuf> = tabs.iter().map(|t| t.path.clone()).collect();

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
        // Drain reload events
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

        // Apply theme
        if self.dark_mode {
            ctx.set_visuals(egui::Visuals::dark());
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }

        // Top panel: tabs + theme toggle
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

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let tab = &mut self.tabs[self.active];
                    CommonMarkViewer::new().show(ui, &mut tab.cache, &tab.content);
                });
        });
    }
}

fn main() {
    let cli = Cli::parse();

    let mut errors = Vec::new();
    for path in &cli.files {
        let is_stdin = path == std::path::Path::new("/dev/stdin");
        if !is_stdin && !path.exists() {
            errors.push(format!("File not found: {}", path.display()));
        } else if !is_stdin && path.extension().and_then(|e| e.to_str()) != Some("md") {
            errors.push(format!("Not a .md file: {}", path.display()));
        }
    }
    if !errors.is_empty() {
        for e in errors {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }

    let tabs: Vec<FileTab> = cli
        .files
        .into_iter()
        .filter_map(|p| match FileTab::load(p) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("error: {e}");
                None
            }
        })
        .collect();

    if tabs.is_empty() {
        eprintln!("error: no files to display");
        std::process::exit(1);
    }

    let (width, height) = if cli.portrait {
        (800.0, 1100.0)
    } else {
        (1200.0, 800.0)
    };

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
