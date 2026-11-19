#![windows_subsystem = "windows"]
//! Tabula Rasa - an uninstaller and nothing else.

mod leftovers;
mod programs;
mod system;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, TextureHandle, Vec2};
use leftovers::Leftover;
use programs::{Program, format_size};

// Colours: dark like the original's menus, with the purple of the Tabula Rasa icon.
const BG: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);
const PANEL: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
const ROW_ALT: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
const LINE: Color32 = Color32::from_rgb(0x45, 0x45, 0x45);
const TEXT: Color32 = Color32::from_rgb(0xea, 0xea, 0xea);
const DIM: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
const PURPLE: Color32 = Color32::from_rgb(0x6c, 0x2a, 0x93);
const PURPLE_LIGHT: Color32 = Color32::from_rgb(0x9b, 0x59, 0xc7);
const WARN: Color32 = Color32::from_rgb(0xe8, 0xb4, 0x4a);
const OK: Color32 = Color32::from_rgb(0x6f, 0xcf, 0x7f);
const BAD: Color32 = Color32::from_rgb(0xf0, 0x6a, 0x6a);

const ROW_H: f32 = 28.0;

enum Msg {
    Programs(u64, Vec<Program>),
    Size(u64, usize, u64),
    Icon(u64, usize, usize, Vec<u8>),
    UninstallDone(u64, Result<u32, String>),
    Leftovers(Vec<Leftover>, Vec<system::RunningProcess>),
    ProcsUpdated(Vec<system::RunningProcess>),
    Removed(Vec<(String, Option<String>)>),
}

#[derive(Clone, Copy, PartialEq)]
enum SortCol {
    Name,
    Publisher,
    Size,
    Date,
    Version,
}

enum Dialog {
    None,
    Confirm { program: Program, scan_after: bool, shared_with: Vec<String> },
    Running { run_id: u64, program: Program, scan_after: bool },
    Waiting { program: Program, exit: Result<u32, String> },
    Scanning { name: String },
    Leftovers { program: Program, items: Vec<Leftover>, running_procs: Vec<system::RunningProcess> },
    Removing { name: String },
    Results { name: String, lines: Vec<(String, Option<String>)> },
    About,
    Message { title: String, text: String },
}

struct App {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    programs: Vec<Program>,
    loading: bool,
    scan_gen: u64,
    uninstall_gen: u64,
    cancel_scan: Arc<AtomicBool>,
    icons: HashMap<usize, TextureHandle>,
    logo: Option<TextureHandle>,
    selected: Option<usize>,
    search: String,
    sort: SortCol,
    ascending: bool,
    show_system: bool,
    show_details: bool,
    dialog: Dialog,
    admin: bool,
    // screenshot mode
    shot_path: Option<PathBuf>,
    frames: u32,
    shot_done: Arc<AtomicBool>,
    shot_requested: bool,
    scroll_to_selected: bool,
    pending_program: Option<Program>,
}

impl App {
    fn new(cc: &eframe::CreationContext, shot_path: Option<PathBuf>) -> Self {
        apply_style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let logo = image::load_from_memory(include_bytes!("../assets/icon-64.png")).ok().map(|img| {
            let img = img.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                egui::TextureOptions::LINEAR,
            )
        });
        let mut app = App {
            tx,
            rx,
            programs: Vec::new(),
            loading: false,
            scan_gen: 0,
            uninstall_gen: 0,
            cancel_scan: Arc::new(AtomicBool::new(false)),
            icons: HashMap::new(),
            logo,
            selected: None,
            search: String::new(),
            sort: SortCol::Name,
            ascending: true,
            show_system: false,
            show_details: true,
            dialog: Dialog::None,
            admin: system::is_admin(),
            shot_path,
            frames: 0,
            shot_done: Arc::new(AtomicBool::new(false)),
            shot_requested: false,
            scroll_to_selected: false,
            pending_program: None,
        };
        app.refresh(&cc.egui_ctx);
        app
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.cancel_scan.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_scan = cancel.clone();
        self.scan_gen = self.scan_gen.wrapping_add(1);
        let scan_id = self.scan_gen;
        self.loading = true;
        self.icons.clear();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let list = programs::scan();
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = tx.send(Msg::Programs(scan_id, list.clone()));
            ctx.request_repaint();
            // Icons
            let tx2 = tx.clone();
            let ctx2 = ctx.clone();
            let cancel2 = cancel.clone();
            let icon_list: Vec<(usize, Option<(String, i32)>)> = list.iter().map(|p| (p.id, p.icon.clone())).collect();
            std::thread::spawn(move || {
                let mut extractor = system::IconExtractor::new();
                let mut pending_repaint = 0usize;
                for (id, icon) in icon_list {
                    if cancel2.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Some((path, idx)) = icon {
                        if let Some((s, rgba)) = extractor.as_mut().and_then(|ex| ex.extract(&path, idx)) {
                            let _ = tx2.send(Msg::Icon(scan_id, id, s, rgba));
                            pending_repaint += 1;
                            if pending_repaint >= 16 {
                                pending_repaint = 0;
                                ctx2.request_repaint();
                            }
                        }
                    }
                }
                if pending_repaint > 0 {
                    ctx2.request_repaint();
                }
            });
            // Sizes of programs that don't state one: measure their folder.
            for p in list.iter().filter(|p| p.size.is_none() && p.install_dir_exists) {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let path = Path::new(&p.install_location);
                if leftovers::is_safe_folder(path) {
                    if let Some(s) = programs::folder_size(path, 300_000) {
                        if s > 0 {
                            let _ = tx.send(Msg::Size(scan_id, p.id, s));
                            ctx.request_repaint();
                        }
                    }
                }
            }
        });
    }

    fn handle_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Programs(scan_id, list) => {
                    if scan_id != self.scan_gen {
                        continue;
                    }
                    let keep = self
                        .selected
                        .and_then(|id| self.programs.get(id))
                        .map(|p| (p.key_name.clone(), p.source));
                    self.programs = list;
                    self.loading = false;
                    self.selected = keep.and_then(|(k, src)| {
                        self.programs
                            .iter()
                            .find(|p| p.key_name == k && p.source == src)
                            .map(|p| p.id)
                    });
                    if self.selected.is_none() && self.shot_path.is_some() {
                        self.selected = self.visible().first().copied();
                    }
                }
                Msg::Size(scan_id, id, s) => {
                    if scan_id == self.scan_gen {
                        if let Some(p) = self.programs.get_mut(id) {
                            p.size = Some(s);
                            p.size_from_folder = true;
                        }
                    }
                }
                Msg::Icon(scan_id, id, s, rgba) => {
                    if scan_id == self.scan_gen {
                        let tex = ctx.load_texture(
                            format!("icon{id}"),
                            egui::ColorImage::from_rgba_unmultiplied([s, s], &rgba),
                            egui::TextureOptions::LINEAR,
                        );
                        self.icons.insert(id, tex);
                    }
                }
                Msg::UninstallDone(run_id, exit) => {
                    if let Dialog::Running { run_id: active_run, program, scan_after } = &self.dialog {
                        if *active_run == run_id {
                            let gone = !program.key_still_exists();
                            let scan_after = *scan_after;
                            let program = program.clone();
                            if gone && scan_after && exit.is_ok() {
                                self.start_leftover_scan(ctx, program);
                            } else {
                                self.dialog = Dialog::Waiting { program, exit };
                            }
                        }
                    }
                }
                Msg::Leftovers(items, procs) => {
                    if let Dialog::Scanning { .. } = self.dialog {
                        if let Some(p) = self.pending_program.take() {
                            self.dialog = Dialog::Leftovers {
                                program: p,
                                items,
                                running_procs: procs,
                            };
                        }
                    }
                }
                Msg::ProcsUpdated(procs) => {
                    if let Dialog::Leftovers { running_procs, .. } = &mut self.dialog {
                        *running_procs = procs;
                    }
                }
                Msg::Removed(lines) => {
                    if let Dialog::Removing { name } = &self.dialog {
                        self.dialog = Dialog::Results { name: name.clone(), lines };
                        self.refresh(ctx);
                    }
                }
            }
        }
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        let mut v: Vec<usize> = self
            .programs
            .iter()
            .filter(|p| self.show_system || !p.system_component)
            .filter(|p| {
                q.is_empty()
                    || p.name_lower.contains(&q)
                    || p.publisher_lower.contains(&q)
            })
            .map(|p| p.id)
            .collect();
        let ps = &self.programs;
        v.sort_by(|&a, &b| {
            let (a, b) = (&ps[a], &ps[b]);
            let o = match self.sort {
                SortCol::Name => a.name_lower.cmp(&b.name_lower),
                SortCol::Publisher => a.publisher_lower.cmp(&b.publisher_lower),
                SortCol::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                SortCol::Date => a.install_date.cmp(&b.install_date),
                SortCol::Version => programs::cmp_version(&a.version, &b.version),
            };
            let o = if self.ascending { o } else { o.reverse() };
            o.then_with(|| a.name_lower.cmp(&b.name_lower))
        });
        v
    }

    fn ask_uninstall(&mut self, id: usize) {
        let Some(p) = self.programs.get(id).cloned() else { return };
        // Warn when another program uses the same folder (as the original does).
        let shared_with: Vec<String> = if p.install_location.is_empty() {
            Vec::new()
        } else {
            let target = Path::new(&p.install_location);
            self.programs
                .iter()
                .filter(|o| o.id != id && !o.install_location.is_empty())
                .filter(|o| leftovers::same_path(Path::new(&o.install_location), target))
                .map(|o| o.name.clone())
                .collect()
        };
        self.dialog = Dialog::Confirm { program: p, scan_after: true, shared_with };
    }

    fn run_uninstaller(&mut self, ctx: &egui::Context, program: Program, scan_after: bool) {
        let (exe, args) = programs::split_command(&program.uninstall_cmd);
        let exe = programs::expand_env(&exe);
        self.uninstall_gen = self.uninstall_gen.wrapping_add(1);
        let run_id = self.uninstall_gen;
        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        self.dialog = Dialog::Running { run_id, program, scan_after };
        std::thread::spawn(move || {
            let res = match system::shell_start(&exe, &args, None) {
                Ok(Some(proc_)) => proc_.wait(),
                Ok(None) => Ok(0),
                Err(e) => Err(e),
            };
            let _ = tx.send(Msg::UninstallDone(run_id, res));
            ctx2.request_repaint();
        });
    }

    fn start_leftover_scan(&mut self, ctx: &egui::Context, p: Program) {
        let others = self.programs.clone();
        self.dialog = Dialog::Scanning { name: p.name.clone() };
        self.pending_program = Some(p.clone());
        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let items = leftovers::scan(&p, &others);
            let procs = if !p.install_location.is_empty() {
                let loc = Path::new(&p.install_location);
                let shared = others.iter().any(|o| {
                    o.id != p.id
                        && !o.install_location.is_empty()
                        && (leftovers::same_path(Path::new(&o.install_location), loc)
                            || leftovers::path_starts_with_ci(Path::new(&o.install_location), loc)
                            || leftovers::path_starts_with_ci(loc, Path::new(&o.install_location)))
                });
                if !shared && leftovers::is_safe_folder(loc) {
                    system::find_processes_in_directory(loc)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            let _ = tx.send(Msg::Leftovers(items, procs));
            ctx2.request_repaint();
        });
    }
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(0x1b, 0x1b, 0x1b);
    v.faint_bg_color = ROW_ALT;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = PURPLE;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = PURPLE_LIGHT;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x3a);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x3a);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x4a, 0x3a, 0x58);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x4a, 0x3a, 0x58);
    v.widgets.active.weak_bg_fill = PURPLE;
    v.widgets.active.bg_fill = PURPLE;
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.style_mut_of(egui::Theme::Dark, |s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        s.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    });
}

fn placeholder_icon(ui: &mut egui::Ui, rect: egui::Rect, name: &str) {
    let painter = ui.painter();
    let hue = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let c = Color32::from_rgb(70 + (hue % 60) as u8, 50 + (hue / 7 % 40) as u8, 100 + (hue / 13 % 70) as u8);
    painter.rect_filled(rect.shrink(2.0), CornerRadius::same(4), c);
    let letter = name.chars().find(|c| c.is_alphanumeric()).unwrap_or('?').to_uppercase().to_string();
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(12.0), Color32::WHITE);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_messages(&ctx);

        // Screenshot option: draw ~120 frames, save the picture, close.
        if let Some(path) = self.shot_path.clone() {
            self.frames += 1;
            ctx.request_repaint();
            if self.shot_done.load(Ordering::SeqCst) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.frames >= 120 && !self.shot_requested && !self.loading {
                self.shot_requested = true;
                let done = self.shot_done.clone();
                ctx.request_screenshot(move |img| {
                    let [w, h] = img.size;
                    let mut bytes = Vec::with_capacity(w * h * 4);
                    for p in &img.pixels {
                        bytes.extend_from_slice(&p.to_srgba_unmultiplied());
                    }
                    let res: Result<(), String> = (|| {
                        if let Some(parent) = path.parent() {
                            if !parent.as_os_str().is_empty() {
                                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                            }
                        }
                        let buf = image::RgbaImage::from_raw(w as u32, h as u32, bytes)
                            .ok_or_else(|| "Invalid RGBA image dimensions".to_string())?;
                        buf.save(&path).map_err(|e| e.to_string())
                    })();
                    if let Err(e) = res {
                        eprintln!("Failed to save screenshot to {}: {e}", path.display());
                        std::process::exit(1);
                    }
                    done.store(true, Ordering::SeqCst);
                });
            }
        }

        // Keyboard shortcuts
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.refresh(&ctx);
        }
        if matches!(self.dialog, Dialog::None) && !ctx.egui_wants_keyboard_input() {
            if let Some(id) = self.selected {
                if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                    self.ask_uninstall(id);
                }
            }
        }

        let vis = self.visible();
        self.menu_bar(ui);
        self.toolbar(ui);
        self.status_bar(ui, &vis);
        if self.show_details {
            self.details_panel(ui);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| self.program_list(ui, &vis));

        self.dialogs(&ctx);
    }
}

impl App {
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(6, 3)))
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Refresh list        F5").clicked() {
                            self.refresh(&ui.ctx().clone());
                            ui.close();
                        }
                        if ui.button("Open backups folder").clicked() {
                            let dir = leftovers::backups_folder();
                            let _ = std::fs::create_dir_all(&dir);
                            system::open_path(&dir.to_string_lossy());
                            ui.close();
                        }
                        if !self.admin && ui.button("Restart as administrator").clicked() {
                            if system::relaunch_as_admin() {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("View", |ui| {
                        ui.checkbox(&mut self.show_system, "Show system components");
                        ui.checkbox(&mut self.show_details, "Show details panel");
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("About Tabula Rasa").clicked() {
                            self.dialog = Dialog::About;
                            ui.close();
                        }
                    });
                });
            });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .stroke(Stroke::new(1.0, LINE)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(36.0)));
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Tabula Rasa").size(19.0).strong().color(Color32::WHITE));
                        ui.label(RichText::new("Clean uninstaller").size(12.0).color(DIM));
                    });
                    ui.add_space(18.0);
                    let has_sel = self.selected.is_some();
                    let uninstall = egui::Button::new(RichText::new("🗑  Uninstall").strong().color(Color32::WHITE))
                        .fill(if has_sel { PURPLE } else { Color32::from_rgb(0x3a, 0x3a, 0x3a) })
                        .min_size(Vec2::new(120.0, 34.0));
                    if ui.add_enabled(has_sel, uninstall).on_hover_text("Uninstall the selected program (Del)").clicked() {
                        if let Some(id) = self.selected {
                            self.ask_uninstall(id);
                        }
                    }
                    if ui.add(egui::Button::new("⟳  Refresh").min_size(Vec2::new(0.0, 34.0))).on_hover_text("Read the list again (F5)").clicked() {
                        self.refresh(&ui.ctx().clone());
                    }
                    let folder = self
                        .selected
                        .and_then(|id| self.programs.get(id))
                        .filter(|p| p.install_dir_exists)
                        .map(|p| p.install_location.clone());
                    if ui.add_enabled(folder.is_some(), egui::Button::new("📂  Open folder").min_size(Vec2::new(0.0, 34.0))).clicked() {
                        if let Some(f) = folder {
                            system::open_path(&f);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let search_w = (ui.available_width() - 8.0).clamp(120.0, 240.0);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("🔍  Search programs")
                                .desired_width(search_w)
                                .min_size(Vec2::new(0.0, 30.0)),
                        );
                        if matches!(self.dialog, Dialog::None)
                            && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::F))
                        {
                            r.request_focus();
                        }
                    });
                });
                if !self.admin {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚠").color(WARN));
                        ui.label(RichText::new("Not running as administrator: some leftovers of programs installed for all users can't be removed.").color(DIM).size(12.5));
                        if ui.small_button("Restart as administrator").clicked() && system::relaunch_as_admin() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, vis: &[usize]) {
        let total: u64 = vis.iter().filter_map(|&i| self.programs[i].size).sum();
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(10, 5)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.loading {
                        ui.spinner();
                        ui.label(RichText::new("Scanning installed programs…").color(DIM));
                    } else {
                        ui.label(RichText::new(format!("Total: {} programs", vis.len())).color(DIM));
                        ui.separator();
                        ui.label(RichText::new(format!("Size: {}", format_size(total))).color(DIM));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mode = if self.admin { "Administrator" } else { "Standard user" };
                        ui.label(RichText::new(mode).color(DIM).size(12.0));
                    });
                });
            });
    }

    fn details_panel(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.selected.and_then(|id| self.programs.get(id)) else { return };
        egui::Panel::bottom("details")
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 10)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                    match self.icons.get(&p.id) {
                        Some(t) => {
                            egui::Image::new(t).paint_at(ui, rect);
                        }
                        None => placeholder_icon(ui, rect, &p.name),
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&p.name).size(16.0).strong().color(Color32::WHITE));
                        let mut sub = Vec::new();
                        if !p.publisher.is_empty() { sub.push(p.publisher.clone()); }
                        if !p.version.is_empty() { sub.push(format!("Version {}", p.version)); }
                        sub.push(p.source.label().to_string());
                        if p.is_msi { sub.push("Windows Installer".into()); }
                        ui.label(RichText::new(sub.join("   ·   ")).color(DIM));
                    });
                });
                ui.add_space(4.0);
                egui::Grid::new("details_grid").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, k: &str, v: &str| {
                        ui.label(RichText::new(k).color(DIM));
                        ui.add(egui::Label::new(if v.is_empty() { "—" } else { v }).truncate());
                        ui.end_row();
                    };
                    row(ui, "Install location", &p.install_location);
                    let date = if p.install_date.is_empty() {
                        String::new()
                    } else if p.date_from_key_time {
                        format!("{} (from the registry entry's last change)", p.install_date)
                    } else {
                        p.install_date.clone()
                    };
                    row(ui, "Installed on", &date);
                    row(ui, "Uninstall command", &p.uninstall_cmd);
                    ui.label(RichText::new("Registry key").color(DIM));
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(p.key_full_path()).truncate());
                    });
                    ui.end_row();
                    if !p.comments.is_empty() {
                        row(ui, "Comments", &p.comments);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Open in Registry Editor").clicked() {
                        system::open_in_regedit(&p.key_full_path());
                    }
                    if let Some(url) = p.website() {
                        if ui.button("Visit website").on_hover_text(url).clicked() {
                            system::open_url(url);
                        }
                    }
                    if ui.button("Copy name").clicked() {
                        ui.ctx().copy_text(p.name.clone());
                    }
                });
            });
    }

    fn program_list(&mut self, ui: &mut egui::Ui, vis: &[usize]) {
        let full_w = ui.available_width();
        let scroll_w = ui.spacing().scroll.allocated_width();
        let avail_w = (full_w - scroll_w).max(200.0);
        let base_size = 90.0_f32;
        let base_date = 100.0_f32;
        let base_ver = 120.0_f32;
        let base_pub = (avail_w * 0.24).clamp(120.0, 300.0);
        let min_name = 150.0_f32;
        let fixed_sum = base_size + base_date + base_ver + base_pub;
        let (name_w, pub_w, size_w, date_w, ver_w) = if avail_w >= fixed_sum + min_name {
            (avail_w - fixed_sum, base_pub, base_size, base_date, base_ver)
        } else {
            let scale = avail_w / (fixed_sum + min_name);
            (min_name * scale, base_pub * scale, base_size * scale, base_date * scale, base_ver * scale)
        };
        let cols = [
            (SortCol::Name, "Program Name", name_w),
            (SortCol::Publisher, "Publisher", pub_w),
            (SortCol::Size, "Size", size_w),
            (SortCol::Date, "Install Date", date_w),
            (SortCol::Version, "Version", ver_w),
        ];

        // Header
        let (hrect, _) = ui.allocate_exact_size(Vec2::new(full_w, 26.0), Sense::hover());
        ui.painter().rect_filled(hrect, CornerRadius::ZERO, PANEL);
        let mut x = hrect.left();
        for (col, title, w) in cols {
            let r = egui::Rect::from_min_size(egui::pos2(x, hrect.top()), Vec2::new(w, hrect.height()));
            let resp = ui.interact(r, ui.scope_id().with(title), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(0x33, 0x2b, 0x3a));
            }
            if resp.clicked() {
                if self.sort == col { self.ascending = !self.ascending; } else { self.sort = col; self.ascending = true; }
            }
            let arrow = if self.sort == col { if self.ascending { " ▲" } else { " ▼" } } else { "" };
            let pad = if col == SortCol::Name { 34.0 } else { 8.0 };
            let text_w = (w - pad - 6.0).max(0.0);
            let mut job = egui::text::LayoutJob::single_section(
                format!("{title}{arrow}"),
                egui::TextFormat::simple(egui::FontId::proportional(13.0), DIM),
            );
            job.wrap = egui::text::TextWrapping {
                max_width: text_w,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let galley = ui.painter().layout_job(job);
            ui.painter_at(r).galley(
                egui::pos2(r.left() + pad, r.center().y - galley.size().y / 2.0),
                galley,
                DIM,
            );
            ui.painter().vline(r.right() - 1.0, r.y_range().shrink(5.0), Stroke::new(1.0, LINE));
            x += w;
        }

        if self.loading && self.programs.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(RichText::new("Reading installed programs…").color(DIM));
            });
            return;
        }
        if vis.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("No programs match your search.").color(DIM)));
            return;
        }

        // Keyboard navigation
        if matches!(self.dialog, Dialog::None) && !ui.ctx().egui_wants_keyboard_input() {
            let pos = self.selected.and_then(|s| vis.iter().position(|&v| v == s));
            let page = ((ui.available_height() / ROW_H).floor() as usize).max(1);
            let (down, up, home, end, pgdn, pgup, enter) = ui.input(|i| (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::Enter),
            ));
            if down {
                self.selected = Some(vis[pos.map_or(0, |p| (p + 1).min(vis.len() - 1))]);
                self.scroll_to_selected = true;
            } else if up {
                self.selected = Some(vis[pos.map_or(0, |p| p.saturating_sub(1))]);
                self.scroll_to_selected = true;
            } else if home {
                self.selected = Some(vis[0]);
                self.scroll_to_selected = true;
            } else if end {
                self.selected = Some(vis[vis.len() - 1]);
                self.scroll_to_selected = true;
            } else if pgdn {
                self.selected = Some(vis[pos.map_or(0, |p| (p + page).min(vis.len() - 1))]);
                self.scroll_to_selected = true;
            } else if pgup {
                self.selected = Some(vis[pos.map_or(0, |p| p.saturating_sub(page))]);
                self.scroll_to_selected = true;
            }
            if enter {
                if let Some(id) = self.selected { self.ask_uninstall(id); }
            }
        }

        let scroll_to_pos = if self.scroll_to_selected {
            self.scroll_to_selected = false;
            self.selected.and_then(|s| vis.iter().position(|&v| v == s))
        } else {
            None
        };

        let mut action: Option<(usize, &'static str)> = None;
        let prev_spacing_y = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, vis.len(), |ui, range| {
            if let Some(p) = scroll_to_pos {
                let target_y = ui.max_rect().top() + (p as isize - range.start as isize) as f32 * ROW_H;
                let target_rect = egui::Rect::from_min_size(
                    egui::pos2(ui.max_rect().left(), target_y),
                    Vec2::new(avail_w, ROW_H),
                );
                ui.scroll_to_rect(target_rect, None);
            }
            for row in range {
                let id = vis[row];
                let p = &self.programs[id];
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail_w, ROW_H), Sense::click());
                let selected = self.selected == Some(id);
                let painter = ui.painter_at(rect);
                if selected {
                    painter.rect_filled(rect, CornerRadius::ZERO, PURPLE);
                } else if resp.hovered() {
                    painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(0x3d, 0x33, 0x45));
                } else if row % 2 == 1 {
                    painter.rect_filled(rect, CornerRadius::ZERO, ROW_ALT);
                }
                let icon_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 8.0, rect.top() + 4.0), Vec2::splat(20.0));
                match self.icons.get(&id) {
                    Some(t) => {
                        egui::Image::new(t).paint_at(ui, icon_rect);
                    }
                    None => placeholder_icon(ui, icon_rect, &p.name),
                }
                let text_col = if selected { Color32::WHITE } else { TEXT };
                let dim_col = if selected { Color32::from_rgb(0xe6, 0xd8, 0xf0) } else { DIM };
                let size_txt = p.size.map(format_size).unwrap_or_else(|| "Unknown".into());
                let cells: [(&str, f32, Color32, f32); 5] = [
                    (&p.name, name_w, text_col, 34.0),
                    (&p.publisher, pub_w, dim_col, 8.0),
                    (&size_txt, size_w, dim_col, 8.0),
                    (&p.install_date, date_w, dim_col, 8.0),
                    (&p.version, ver_w, dim_col, 8.0),
                ];
                let mut x = rect.left();
                let mut any_elided = false;
                for (txt, w, col, pad) in cells {
                    let cell_w = (w - pad - 6.0).max(0.0);
                    let cell = egui::Rect::from_min_size(egui::pos2(x + pad, rect.top()), Vec2::new(cell_w, ROW_H));
                    let mut job = egui::text::LayoutJob::single_section(
                        txt.to_string(),
                        egui::TextFormat::simple(egui::FontId::proportional(13.5), col),
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: cell_w,
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let galley = ui.painter().layout_job(job);
                    any_elided |= galley.elided;
                    let clip = ui.painter_at(cell);
                    clip.galley(egui::pos2(cell.left(), cell.center().y - galley.size().y / 2.0), galley, col);
                    x += w;
                }
                if resp.clicked() {
                    self.selected = Some(id);
                }
                if resp.double_clicked() {
                    action = Some((id, "uninstall"));
                }
                resp.context_menu(|ui| {
                    self.selected = Some(id);
                    if ui.button("🗑  Uninstall").clicked() { action = Some((id, "uninstall")); ui.close(); }
                    if ui.button("📂  Open install folder").clicked() { action = Some((id, "folder")); ui.close(); }
                    if ui.button("Open in Registry Editor").clicked() { action = Some((id, "regedit")); ui.close(); }
                    if ui.button("Visit website").clicked() { action = Some((id, "web")); ui.close(); }
                    if ui.button("Copy name").clicked() { action = Some((id, "copy")); ui.close(); }
                });
                if any_elided && resp.hovered() {
                    let mut tip = p.name.clone();
                    if !p.publisher.is_empty() {
                        tip.push_str(&format!("\n{}", p.publisher));
                    }
                    if !p.version.is_empty() {
                        tip.push_str(&format!("\nVersion {}", p.version));
                    }
                    resp.on_hover_text(tip);
                }
            }
        });
        ui.spacing_mut().item_spacing.y = prev_spacing_y;
        if let Some((id, what)) = action {
            let p = &self.programs[id];
            match what {
                "uninstall" => self.ask_uninstall(id),
                "folder" => {
                    if p.install_dir_exists {
                        system::open_path(&p.install_location);
                    } else {
                        self.dialog = Dialog::Message { title: "No folder".into(), text: format!("{} has no install folder on this computer.", p.name) };
                    }
                }
                "regedit" => system::open_in_regedit(&p.key_full_path()),
                "web" => match p.website() {
                    Some(u) => system::open_url(u),
                    None => self.dialog = Dialog::Message { title: "No website".into(), text: format!("{} doesn't list a website.", p.name) },
                },
                "copy" => ui.ctx().copy_text(p.name.clone()),
                _ => {}
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if matches!(self.dialog, Dialog::None) {
            return;
        }
        let frame = egui::Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0, PURPLE))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(18));
        let mut next: Option<Dialog> = None;
        let mut start_uninstall: Option<(Program, bool)> = None;
        let mut start_scan: Option<Program> = None;
        let mut start_kill_procs: Option<(Vec<u32>, String)> = None;
        let mut start_remove: Option<(String, Vec<Leftover>)> = None;
        let mut close_on_escape = false;

        let dialog = &mut self.dialog;
        egui::Modal::new(egui::Id::unique("tabula_dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(520.0);
            match dialog {
                Dialog::Confirm { program, scan_after, shared_with } => {
                    ui.label(RichText::new("Uninstall program").size(18.0).strong());
                    ui.add_space(6.0);
                    ui.label(format!("Do you want to uninstall {}?", program.name));
                    if !shared_with.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!(
                            "⚠ Its folder is also used by: {}. Leftovers in that folder will be kept.",
                            shared_with.join(", ")
                        )).color(WARN));
                    }
                    ui.add_space(6.0);
                    ui.checkbox(scan_after, "Look for leftovers when it's done");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Uninstall").strong().color(Color32::WHITE)).fill(PURPLE)).clicked() {
                            start_uninstall = Some((program.clone(), *scan_after));
                        }
                        if ui.button("Cancel").clicked() { next = Some(Dialog::None); }
                    });
                    close_on_escape = true;
                }
                Dialog::Running { run_id: _, program, .. } => {
                    ui.label(RichText::new("Uninstalling…").size(18.0).strong());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("The uninstaller of {} is running. Follow its steps; Tabula Rasa waits for it.", program.name));
                    });
                    ui.add_space(10.0);
                    if ui.button("Stop waiting").clicked() {
                        next = Some(Dialog::Waiting { program: program.clone(), exit: Ok(0) });
                    }
                }
                Dialog::Waiting { program, exit } => {
                    ui.label(RichText::new("Uninstaller closed").size(18.0).strong());
                    ui.add_space(6.0);
                    match exit {
                        Err(e) => { ui.label(RichText::new(e.as_str()).color(BAD)); }
                        Ok(_) if !program.key_still_exists() => { ui.label(RichText::new(format!("{} was removed.", program.name)).color(OK)); }
                        Ok(_) => {
                            ui.label(format!("{} still shows as installed.", program.name));
                            ui.label(RichText::new("Some uninstallers keep running in a new window. When it has finished, look for leftovers. If you cancelled it, just close this.").color(DIM));
                        }
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Look for leftovers").strong().color(Color32::WHITE)).fill(PURPLE)).clicked() {
                            start_scan = Some(program.clone());
                        }
                        if ui.button("Close").clicked() { next = Some(Dialog::None); }
                    });
                    close_on_escape = true;
                }
                Dialog::Scanning { name } => {
                    ui.label(RichText::new("Looking for leftovers").size(18.0).strong());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| { ui.spinner(); ui.label(format!("Searching for files and settings {name} left behind…")); });
                }
                Dialog::Leftovers { program, items, running_procs } => {
                    ui.label(RichText::new("Leftovers").size(18.0).strong());
                    ui.add_space(4.0);
                    if items.is_empty() {
                        ui.label(RichText::new(format!("Nothing left behind by {}. All clean.", program.name)).color(OK));
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() { next = Some(Dialog::None); }
                        close_on_escape = true;
                    } else {
                        ui.label(format!("{} left these behind. Tick what to remove:", program.name));
                        if !running_procs.is_empty() {
                            ui.add_space(6.0);
                            let proc_names: Vec<String> = running_procs
                                .iter()
                                .map(|p| format!("{} (PID {})", p.name, p.pid))
                                .collect();
                            ui.label(
                                RichText::new(format!(
                                    "⚠ Running process(es) detected: {}. They may lock files during removal.",
                                    proc_names.join(", ")
                                ))
                                .color(WARN)
                                .size(12.5),
                            );
                            if ui
                                .add(egui::Button::new("Close running processes").fill(Color32::from_rgb(0x7a, 0x4a, 0x22)))
                                .clicked()
                            {
                                let pids = running_procs.iter().map(|p| p.pid).collect();
                                start_kill_procs = Some((pids, program.install_location.clone()));
                            }
                        }
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                            for it in items.iter_mut() {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut it.selected, "");
                                    ui.label(RichText::new(it.kind.label()).color(PURPLE_LIGHT).size(12.0));
                                    let size = it.size.map(|s| format!("  ({})", format_size(s))).unwrap_or_default();
                                    ui.add(egui::Label::new(format!("{}{}", it.display_path(), size)).truncate()).on_hover_text(it.reason);
                                });
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(RichText::new("Files go to the Recycle Bin. Registry keys are saved to a backup first (File → Open backups folder).").color(DIM).size(12.0));
                        ui.add_space(8.0);
                        let any = items.iter().any(|i| i.selected);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(any, egui::Button::new(RichText::new("Remove selected").strong().color(Color32::WHITE)).fill(PURPLE)).clicked() {
                                start_remove = Some((program.name.clone(), items.clone()));
                            }
                            if ui.button("Select all").clicked() { items.iter_mut().for_each(|i| i.selected = true); }
                            if ui.button("Select none").clicked() { items.iter_mut().for_each(|i| i.selected = false); }
                            if ui.button("Skip").clicked() { next = Some(Dialog::None); }
                        });
                        close_on_escape = true;
                    }
                }
                Dialog::Removing { name } => {
                    ui.label(RichText::new("Cleaning up").size(18.0).strong());
                    ui.horizontal(|ui| { ui.spinner(); ui.label(format!("Removing what {name} left behind…")); });
                }
                Dialog::Results { name, lines } => {
                    let failed = lines.iter().filter(|l| l.1.is_some()).count();
                    ui.label(RichText::new("Done").size(18.0).strong());
                    ui.add_space(4.0);
                    if failed == 0 {
                        ui.label(RichText::new(format!("{name} is gone. {} item(s) removed.", lines.len())).color(OK));
                    } else {
                        ui.label(RichText::new(format!("{} removed, {failed} could not be removed.", lines.len() - failed)).color(WARN));
                    }
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for (path, err) in lines.iter() {
                            match err {
                                None => ui.label(RichText::new(format!("✔ {path}")).color(DIM).size(12.5)),
                                Some(e) => ui.label(RichText::new(format!("✖ {path} — {e}")).color(BAD).size(12.5)),
                            };
                        }
                    });
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() { next = Some(Dialog::None); }
                    close_on_escape = true;
                }
                Dialog::About => {
                    ui.label(RichText::new("Tabula Rasa").size(20.0).strong());
                    ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(DIM));
                    ui.add_space(6.0);
                    ui.label("A clean slate for your computer: remove programs and what they leave behind.");
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() { next = Some(Dialog::None); }
                    close_on_escape = true;
                }
                Dialog::Message { title, text } => {
                    ui.label(RichText::new(title.as_str()).size(18.0).strong());
                    ui.add_space(6.0);
                    ui.label(text.as_str());
                    ui.add_space(10.0);
                    if ui.button("OK").clicked() { next = Some(Dialog::None); }
                    close_on_escape = true;
                }
                Dialog::None => {}
            }
        });
        if close_on_escape && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            next = Some(Dialog::None);
        }
        if let Some(d) = next {
            self.dialog = d;
        }
        if let Some((program, scan)) = start_uninstall {
            self.run_uninstaller(ctx, program, scan);
        }
        if let Some(program) = start_scan {
            self.start_leftover_scan(ctx, program);
        }
        if let Some((pids, dir)) = start_kill_procs {
            let tx = self.tx.clone();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                for pid in pids {
                    system::terminate_process_by_pid(pid);
                }
                let procs = if !dir.is_empty() && leftovers::is_safe_folder(Path::new(&dir)) {
                    system::find_processes_in_directory(Path::new(&dir))
                } else {
                    Vec::new()
                };
                let _ = tx.send(Msg::ProcsUpdated(procs));
                ctx2.request_repaint();
            });
        }
        if let Some((name, items)) = start_remove {
            self.dialog = Dialog::Removing { name: name.clone() };
            let tx = self.tx.clone();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let lines = leftovers::remove(&items, &name);
                let _ = tx.send(Msg::Removed(lines));
                ctx2.request_repaint();
            });
        }
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let shot_path = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let icon = image::load_from_memory(include_bytes!("../assets/icon-256.png"))
        .ok()
        .map(|img| {
            let img = img.to_rgba8();
            egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
        });

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Tabula Rasa")
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([760.0, 480.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    if shot_path.is_some() {
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Tabula Rasa", options, Box::new(move |cc| Ok(Box::new(App::new(cc, shot_path)))))
}
