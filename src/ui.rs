//! egui desktop UI for the local LLM manager.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use serde_json::Value;

use crate::config::{Config, ServiceDef};
use crate::library::{self, ModelEntry};
use crate::runtime::{self, StateStore};
use crate::util;

const GREEN: Color32 = Color32::from_rgb(72, 187, 120);
const RED: Color32 = Color32::from_rgb(235, 94, 85);
const AMBER: Color32 = Color32::from_rgb(230, 170, 60);
const GREY: Color32 = Color32::from_rgb(140, 148, 160);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Models,
    Services,
    Status,
    Logs,
    Settings,
}

#[derive(Default, Clone)]
struct Status {
    running: bool,
    pid: u32,
    uptime: u64,
    health: Option<Value>,
    props: Option<Value>,
    rss: Option<u64>,
    metrics: HashMap<String, f64>,
    error: Option<String>,
}

enum Action {
    Start(String),
    Stop(String),
    Restart(String),
    RemoveService(String),
    DeleteModel { files: Vec<PathBuf>, size: u64 },
    SaveConfig,
    Rescan,
    Reveal(PathBuf),
}

struct Editor {
    original_id: Option<String>,
    name: String,
    model: PathBuf,
    mmproj: Option<PathBuf>,
    port: u16,
    ctx: u32,
    ngl: i32,
    parallel: u32,
    image_min_tokens: u32,
    extra: String,
}

struct Confirm {
    title: String,
    body: String,
    detail: Vec<String>,
    files: Vec<PathBuf>,
    size: u64,
    service: Option<String>,
}

pub struct App {
    config: Config,
    state: StateStore,
    models: Vec<ModelEntry>,
    scan_errors: Vec<String>,
    tab: Tab,
    status: HashMap<String, Status>,
    filter: String,
    toast: Option<(String, bool, Instant)>,
    editor: Option<Editor>,
    confirm: Option<Confirm>,
    log_for: Option<String>,
    log_lines: Vec<String>,
    last_poll: Instant,
    server_missing_warned: bool,
    sys: runtime::SysInfo,
}

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("LLM Manager — 本地大模型管理"),
        ..Default::default()
    };
    eframe::run_native(
        "LLM Manager",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

/// macOS ships CJK fonts as `.ttc` collections; load one as a fallback face.
fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    for path in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.len() < 4 {
            continue;
        }
        let mut fonts = egui::FontDefinitions::default();
        let mut data = egui::FontData::from_owned(bytes);
        data.index = 0;
        fonts
            .font_data
            .insert("cjk".to_owned(), Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
}

impl App {
    fn new() -> Self {
        let config = Config::load();
        let mut state = StateStore::load();
        state.prune();
        let (models, scan_errors) = library::scan(&config.model_dirs);
        let tab = match std::env::var("LLMMGR_TAB").as_deref() {
            Ok("services") => Tab::Services,
            Ok("status") => Tab::Status,
            Ok("logs") => Tab::Logs,
            Ok("settings") => Tab::Settings,
            _ => Tab::Models,
        };
        let mut app = Self {
            config,
            state,
            models,
            scan_errors,
            tab,
            status: HashMap::new(),
            filter: String::new(),
            toast: None,
            editor: None,
            confirm: None,
            log_for: None,
            log_lines: Vec::new(),
            last_poll: Instant::now() - Duration::from_secs(60),
            server_missing_warned: false,
            sys: runtime::sys_info(),
        };
        app.poll();
        app
    }

    fn toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast = Some((message.into(), error, Instant::now()));
    }

    /// Refresh process/health state for every configured service.
    fn poll(&mut self) {
        self.last_poll = Instant::now();
        self.sys = runtime::sys_info();
        if let Some(id) = self.log_for.clone() {
            if let Some(run) = self.state.services.get(&id) {
                if runtime::is_alive(run.pid) {
                    self.log_lines = runtime::log_tail(&run.log, 400);
                }
            }
        }
        self.state.prune();
        for service in &self.config.services {
            let mut status = Status::default();
            if let Some(run) = self.state.services.get(&service.id) {
                if runtime::is_alive(run.pid) {
                    status.running = true;
                    status.pid = run.pid;
                    status.uptime = util::now_unix().saturating_sub(run.started_at);
                    status.rss = runtime::rss_bytes(run.pid);
                    match runtime::health(&service.host, service.port) {
                        Ok(value) => {
                            status.health = Some(value);
                            if let Ok(props) = runtime::props(&service.host, service.port) {
                                status.props = Some(props);
                            }
                            if let Ok(text) = runtime::metrics_text(&service.host, service.port) {
                                status.metrics = parse_metrics(&text);
                            }
                        }
                        Err(err) => status.error = Some(err),
                    }
                }
            }
            self.status.insert(service.id.clone(), status);
        }
        if let Some(id) = self.log_for.clone() {
            if let Some(run) = self.state.services.get(&id) {
                self.log_lines = runtime::log_tail(&run.log, 400);
            }
        }
    }

    fn rescan(&mut self) {
        let (models, errors) = library::scan(&self.config.model_dirs);
        self.models = models;
        self.scan_errors = errors;
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Start(id) => {
                if let Some(service) = self.config.service(&id).cloned() {
                    match runtime::start(&self.config, &service, &mut self.state) {
                        Ok(pid) => self.toast(format!("已启动 {} (pid {pid})", service.name), false),
                        Err(err) => self.toast(format!("启动失败: {err}"), true),
                    }
                }
            }
            Action::Stop(id) => {
                let name = self.config.service(&id).map(|s| s.name.clone()).unwrap_or_default();
                match runtime::stop(&id, &mut self.state) {
                    Ok(()) => self.toast(format!("已停止 {name}"), false),
                    Err(err) => self.toast(format!("停止失败: {err}"), true),
                }
            }
            Action::Restart(id) => {
                let _ = runtime::stop(&id, &mut self.state);
                std::thread::sleep(Duration::from_millis(300));
                self.apply(Action::Start(id));
            }
            Action::RemoveService(id) => {
                let _ = runtime::stop(&id, &mut self.state);
                self.config.services.retain(|s| s.id != id);
                self.status.remove(&id);
                if self.log_for.as_deref() == Some(id.as_str()) {
                    self.log_for = None;
                    self.log_lines.clear();
                }
                match self.config.save() {
                    Ok(()) => self.toast(format!("已删除服务 {id}"), false),
                    Err(err) => self.toast(format!("保存配置失败: {err}"), true),
                }
            }
            Action::DeleteModel { files, size } => {
                match library::delete_files(&files) {
                    Ok(freed) => {
                        self.toast(
                            format!("已删除 {} 个文件，释放 {}", files.len(), util::human_bytes(freed)),
                            false,
                        );
                        self.rescan();
                    }
                    Err(err) => self.toast(format!("删除失败: {err} ({} 释放)", util::human_bytes(size)), true),
                }
            }
            Action::SaveConfig => match self.config.save() {
                Ok(()) => self.toast("配置已保存", false),
                Err(err) => self.toast(format!("保存失败: {err}"), true),
            },
            Action::Rescan => {
                self.rescan();
                self.poll();
                self.toast("已刷新", false);
            }
            Action::Reveal(path) => {
                let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
            }
        }
    }

    // ------------------------------------------------------------------ panels

    fn top_bar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("LLM Manager");
                ui.label(
                    RichText::new("本地大模型管理")
                        .color(GREY)
                        .size(13.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("刷新").clicked() {
                        actions.push(Action::Rescan);
                    }
                    let running = self.status.values().filter(|s| s.running).count();
                    let total_size: u64 = self.models.iter().map(|m| m.total_size()).sum();
                    ui.label(
                        RichText::new(format!(
                            "{} 个模型 · {} · {} 个服务运行中",
                            self.models.len(),
                            util::human_bytes(total_size),
                            running
                        ))
                        .color(GREY),
                    );

                });
            });
            ui.add_space(4.0);
        });
    }

    fn nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(150.0)
            .show(ctx, |ui| {
                ui.add_space(10.0);
                for (tab, label) in [
                    (Tab::Models, "模型库"),
                    (Tab::Services, "服务"),
                    (Tab::Status, "运行状态"),
                    (Tab::Logs, "日志"),
                    (Tab::Settings, "设置"),
                ] {
                    let selected = self.tab == tab;
                    let text = RichText::new(label).size(15.0);
                    let text = if selected { text.strong().color(Color32::WHITE) } else { text };
                    let button = egui::Button::new(text)
                        .fill(if selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            Color32::TRANSPARENT
                        })
                        .min_size(egui::vec2(130.0, 32.0));
                    if ui.add(button).clicked() {
                        self.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(8.0);
                    let server_ok = self
                        .config
                        .llama_server
                        .as_ref()
                        .map(|p| p.is_file())
                        .unwrap_or(false)
                        || crate::config::find_llama_server().is_some();
                    if !server_ok {
                        ui.colored_label(AMBER, "⚠ 未找到 llama-server");
                        if ui.small_button("去设置").clicked() {
                            self.tab = Tab::Settings;
                        }
                    }
                });
            });
    }

    fn models_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.label("搜索");
            ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("模型名 / 量化").desired_width(220.0));
            if ui.button("打开模型目录").clicked() {
                if let Some(dir) = self.config.model_dirs.first() {
                    actions.push(Action::Reveal(dir.clone()));
                }
            }
            ui.label(
                RichText::new(format!("{} 个模型", self.models.len()))
                    .color(GREY),
            );
        });
        if self.config.model_dirs.is_empty() {
            ui.add_space(10.0);
            ui.colored_label(AMBER, "还没有配置模型目录。到「设置」里添加包含 .gguf 文件的目录。");
            return;
        }
        if !self.scan_errors.is_empty() {
            ui.add_space(4.0);
            for err in self.scan_errors.clone() {
                ui.colored_label(AMBER, format!("· {err}"));
            }
        }
        ui.add_space(6.0);
        let running_models: Vec<PathBuf> = self
            .config
            .services
            .iter()
            .filter(|s| self.status.get(&s.id).map(|st| st.running).unwrap_or(false))
            .map(|s| s.model.clone())
            .collect();
        let filter = self.filter.to_ascii_lowercase();
        let models: Vec<ModelEntry> = self
            .models
            .iter()
            .filter(|m| {
                filter.is_empty()
                    || m.file_name.to_ascii_lowercase().contains(&filter)
                    || m.quant().to_ascii_lowercase().contains(&filter)
                    || m.arch().to_ascii_lowercase().contains(&filter)
            })
            .cloned()
            .collect();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("models_grid")
                    .num_columns(7)
                    .striped(true)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        for heading in ["", "模型", "大小", "参数", "量化", "架构", ""] {
                            ui.label(RichText::new(heading).strong().color(GREY));
                        }
                        ui.end_row();
                        for model in &models {
                            if running_models.contains(&model.path) {
                                ui.colored_label(GREEN, RichText::new("●").size(12.0))
                                    .on_hover_text("该模型有服务正在运行");
                            } else {
                                ui.label("");
                            }
                            let name = util::truncate(&model.display_name(), 46);
                            let label = ui.label(RichText::new(name).size(14.0));
                            if model.meta.is_none() {
                                label.on_hover_text(
                                    model.error.clone().unwrap_or_else(|| "无法读取元数据".into()),
                                );
                            } else if let Some(meta) = &model.meta {
                                let modified = model
                                    .modified
                                    .map(|ts| {
                                        let secs = util::now_unix().saturating_sub(ts);
                                        format!("修改于 {} 前", util::duration_label(secs))
                                    })
                                    .unwrap_or_default();
                                label.on_hover_text(format!(
                                    "{}\nGGUF v{} · {} 张量 · 上下文 {} · {} 层 · 嵌入 {}\n{}\n{}",
                                    model.path.display(),
                                    meta.version,
                                    meta.tensor_count,
                                    meta.context_length,
                                    meta.block_count,
                                    meta.embedding_length,
                                    model.file_name,
                                    modified
                                ));
                            }
                            ui.label(util::human_bytes(model.total_size()));
                            ui.label(model.params());
                            let quant = model.quant();
                            let color = if quant.contains("Q4") || quant.contains("Q5") || quant.contains("Q6") {
                                GREEN
                            } else if quant.contains("Q8") || quant.contains("F16") || quant.contains("BF16") {
                                AMBER
                            } else {
                                GREY
                            };
                            ui.colored_label(color, quant);
                            ui.label(model.arch());
                            ui.horizontal(|ui| {
                                if ui.button("启动").clicked() {
                                    let mut editor = Editor {
                                        original_id: None,
                                        name: model.display_name(),
                                        model: model.path.clone(),
                                        mmproj: model.mmproj.clone(),
                                        port: self.config.next_free_port(),
                                        ctx: 4096,
                                        ngl: 99,
                                        parallel: 1,
                                        image_min_tokens: 1024,
                                        extra: String::new(),
                                    };
                                    if let Some(meta) = &model.meta {
                                        if meta.context_length >= 2048 {
                                            editor.ctx = meta.context_length.min(8192) as u32;
                                        }
                                    }
                                    self.editor = Some(editor);
                                }
                                if ui.button("访达").clicked() {
                                    actions.push(Action::Reveal(model.path.clone()));
                                }
                                if ui
                                    .button(RichText::new("删除").color(RED))
                                    .on_hover_text("移到废纸篓")
                                    .clicked()
                                {
                                    let mut detail = Vec::new();
                                    for file in model.all_files() {
                                        detail.push(file.display().to_string());
                                    }
                                    self.confirm = Some(Confirm {
                                        title: "删除模型".into(),
                                        body: format!(
                                            "确定删除 {} 及其 {} 个文件？共 {}。",
                                            model.display_name(),
                                            model.all_files().len(),
                                            util::human_bytes(model.total_size())
                                        ),
                                        detail,
                                        files: model.all_files(),
                                        size: model.total_size(),
                                        service: None,
                                    });
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
    }

    fn services_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            if ui.button("+ 新建服务").clicked() {
                let port = self.config.next_free_port();
                let model = self.models.first().map(|m| m.path.clone()).unwrap_or_default();
                self.editor = Some(Editor {
                    original_id: None,
                    name: "新服务".into(),
                    model,
                    mmproj: None,
                    port,
                    ctx: 4096,
                    ngl: 99,
                    parallel: 1,
                    image_min_tokens: 1024,
                    extra: String::new(),
                });
            }
            if ui.button("全部停止").clicked() {
                for service in self.config.services.clone() {
                    if self
                        .status
                        .get(&service.id)
                        .map(|s| s.running)
                        .unwrap_or(false)
                    {
                        actions.push(Action::Stop(service.id.clone()));
                    }
                }
            }
            ui.label(RichText::new(format!("{} 个服务", self.config.services.len())).color(GREY));
        });
        ui.add_space(6.0);

        if self.config.services.is_empty() {
            ui.colored_label(GREY, "还没有服务。点「新建服务」，或在「模型库」里对某个模型点「启动」。");
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let services = self.config.services.clone();
                for service in &services {
                    let status = self.status.get(&service.id).cloned().unwrap_or_default();
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, color, label) = if status.running {
                                match &status.health {
                                    Some(h) if h.get("status").and_then(|v| v.as_str()) == Some("ok") => {
                                        ("●", GREEN, "运行中")
                                    }
                                    Some(_) => ("●", AMBER, "加载中"),
                                    None => ("●", AMBER, "启动中"),
                                }
                            } else {
                                ("○", GREY, "已停止")
                            };
                            ui.colored_label(color, RichText::new(dot).size(16.0));
                            ui.label(RichText::new(&service.name).size(15.0).strong());
                            ui.colored_label(color, label);
                            ui.label(RichText::new(format!("{}:{}", service.host, service.port)).color(GREY));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(RichText::new("删除").color(RED)).clicked() {
                                    self.confirm = Some(Confirm {
                                        title: "删除服务".into(),
                                        body: format!("删除服务「{}」？不会删除模型文件。", service.name),
                                        detail: vec![format!("端口 {}", service.port)],
                                        files: Vec::new(),
                                        size: 0,
                                        service: Some(service.id.clone()),
                                    });
                                }
                                if ui.button("编辑").clicked() {
                                    self.editor = Some(Editor::from_service(service, &self.config));
                                }
                                if status.running {
                                    if ui.button("重启").clicked() {
                                        actions.push(Action::Restart(service.id.clone()));
                                    }
                                    if ui.button("停止").clicked() {
                                        actions.push(Action::Stop(service.id.clone()));
                                    }
                                } else if ui.button("启动").clicked() {
                                    actions.push(Action::Start(service.id.clone()));
                                }
                                if ui.button("日志").clicked() {
                                    self.tab = Tab::Logs;
                                    self.log_for = Some(service.id.clone());
                                    if let Some(run) = self.state.services.get(&service.id) {
                                        self.log_lines = runtime::log_tail(&run.log, 400);
                                    }
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(util::truncate(&service.model.display().to_string(), 70)).color(GREY));
                            if service.mmproj.is_some() {
                                ui.label(RichText::new("视觉").color(GREEN));
                            }
                        });
                        if status.running {
                            let mut parts = vec![format!("pid {}", status.pid)];
                            parts.push(format!("运行 {}", util::duration_label(status.uptime)));
                            if let Some(rss) = status.rss {
                                parts.push(format!("内存 {}", util::human_bytes(rss)));
                            }
                            if !status.metrics.is_empty() {
                                if let Some(t) = status.metrics.get("llamacpp:tokens_predicted_total") {
                                    parts.push(format!("生成 {:.0} tokens", t));
                                }
                                if let Some(s) = status.metrics.get("llamacpp:requests_processing").or_else(|| status.metrics.get("llamacpp:slots_active")) {
                                    parts.push(format!("处理中 {:.0}", s));
                                }
                            }
                            ui.label(RichText::new(parts.join(" · ")).color(GREY).size(12.0));
                        }
                        if let Some(err) = &status.error {
                            ui.colored_label(AMBER, format!("健康检查: {err}"));
                        }
                    });
                    ui.add_space(6.0);
                }
            });
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("刷新").clicked() {
                self.last_poll = Instant::now() - Duration::from_secs(60);
            }
            ui.label(RichText::new("每 2 秒自动刷新").color(GREY));
        });
        ui.add_space(6.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("系统").strong());
                let used = self.sys.total_ram.saturating_sub(self.sys.free_ram);
                ui.label(format!("内存 {} / {}", util::human_bytes(used), util::human_bytes(self.sys.total_ram)));
                ui.separator();
                ui.label(format!(
                    "磁盘 {} / {}",
                    util::human_bytes(self.sys.disk_free),
                    util::human_bytes(self.sys.disk_total)
                ));
                ui.separator();
                ui.label(format!("{} 个模型, {} 个服务", self.models.len(), self.config.services.len()));
            });
        });
        ui.add_space(6.0);

        let running: Vec<(ServiceDef, Status)> = self
            .config
            .services
            .iter()
            .filter_map(|s| {
                let status = self.status.get(&s.id)?;
                if status.running {
                    Some((s.clone(), status.clone()))
                } else {
                    None
                }
            })
            .collect();
        if running.is_empty() {
            ui.colored_label(GREY, "当前没有运行中的服务。");
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (service, status) in &running {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&service.name).size(15.0).strong());
                        let healthy = status
                            .health
                            .as_ref()
                            .and_then(|h| h.get("status").and_then(|v| v.as_str()))
                            .map(|s| s == "ok")
                            .unwrap_or(false);
                        ui.colored_label(if healthy { GREEN } else { AMBER }, if healthy { "健康" } else { "加载中" });
                        ui.label(RichText::new(format!("http://{}:{}", service.host, service.port)).color(GREY));
                    });
                    ui.add_space(4.0);
                    egui::Grid::new(format!("metrics_{}", service.id))
                        .num_columns(4)
                        .spacing([24.0, 4.0])
                        .show(ui, |ui| {
                            ui.label(RichText::new("进程").color(GREY));
                            ui.label(status.pid.to_string());
                            ui.label(RichText::new("运行时长").color(GREY));
                            ui.label(util::duration_label(status.uptime));
                            ui.end_row();
                            ui.label(RichText::new("内存").color(GREY));
                            ui.label(status.rss.map(util::human_bytes).unwrap_or_else(|| "?".into()));
                            ui.label(RichText::new("模型").color(GREY));
                            ui.label(util::truncate(&service.model.display().to_string(), 52));
                            ui.end_row();
                            if let Some(props) = &status.props {
                                let n_ctx = props
                                    .get("default_generation_settings")
                                    .and_then(|v| v.get("n_ctx"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let slots = props.get("total_slots").and_then(|v| v.as_u64()).unwrap_or(0);
                                ui.label(RichText::new("上下文").color(GREY));
                                ui.label(n_ctx.to_string());
                                ui.label(RichText::new("并行槽").color(GREY));
                                ui.label(slots.to_string());
                                ui.end_row();
                            }
                            let tokens = status.metrics.get("llamacpp:tokens_predicted_total").copied().unwrap_or(0.0);
                            let prompt = status.metrics.get("llamacpp:tokens_evaluated_total")
                                .or_else(|| status.metrics.get("llamacpp:prompt_tokens_total"))
                                .copied()
                                .unwrap_or(0.0);
                            let kv = status.metrics.get("llamacpp:kv_cache_usage_ratio").copied();
                            ui.label(RichText::new("已生成").color(GREY));
                            ui.label(format!("{tokens:.0} tokens"));
                            ui.label(RichText::new("已处理输入").color(GREY));
                            ui.label(format!("{prompt:.0} tokens"));
                            ui.end_row();
                            if let Some(kv) = kv {
                                ui.label(RichText::new("KV 占用").color(GREY));
                                ui.add(egui::ProgressBar::new(kv as f32).desired_width(160.0).show_percentage());
                                ui.label("");
                                ui.label("");
                                ui.end_row();
                            }
                        });
                    if let Some(health) = &status.health {
                        ui.add_space(4.0);
                        ui.collapsing("健康检查原始响应", |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut pretty(health))
                                    .code_editor()
                                    .desired_rows(6),
                            );
                        });
                    }
                    if let Some(err) = &status.error {
                        ui.colored_label(AMBER, format!("健康检查失败: {err}"));
                    }
                });
                ui.add_space(8.0);
            }
        });
    }

    fn logs_tab(&mut self, ui: &mut egui::Ui) {
        if self.log_for.is_none() {
            if let Some(first) = self.config.services.first() {
                self.log_for = Some(first.id.clone());
                if let Some(run) = self.state.services.get(&first.id) {
                    self.log_lines = runtime::log_tail(&run.log, 400);
                }
            }
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("log_service")
                .selected_text(
                    self.log_for
                        .as_ref()
                        .and_then(|id| self.config.service(id))
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "选择服务".into()),
                )
                .show_ui(ui, |ui| {
                    let services = self.config.services.clone();
                    for service in &services {
                        let selected = self.log_for.as_deref() == Some(service.id.as_str());
                        if ui.selectable_label(selected, &service.name).clicked() {
                            self.log_for = Some(service.id.clone());
                            if let Some(run) = self.state.services.get(&service.id) {
                                self.log_lines = runtime::log_tail(&run.log, 400);
                            }
                        }
                    }
                });
            if ui.button("刷新日志").clicked() {
                if let Some(id) = &self.log_for {
                    if let Some(run) = self.state.services.get(id) {
                        self.log_lines = runtime::log_tail(&run.log, 400);
                    }
                }
            }
            if ui.button("打开日志文件").clicked() {
                if let Some(id) = &self.log_for {
                    if let Some(run) = self.state.services.get(id) {
                        let _ = std::process::Command::new("open").arg("-R").arg(&run.log).spawn();
                    }
                }
            }
        });
        ui.add_space(4.0);
        let mut text = self.log_lines.join("\n");
        egui::ScrollArea::both().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30),
            );
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading("模型目录");
            ui.label(RichText::new("这些目录下的 .gguf 文件会被扫描").color(GREY));
            ui.add_space(4.0);
            let dirs = self.config.model_dirs.clone();
            for dir in &dirs {
                ui.horizontal(|ui| {
                    ui.label(dir.display().to_string());
                    if ui.small_button("移除").clicked() {
                        self.config.model_dirs.retain(|d| d != dir);
                        self.rescan();
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.button("+ 添加目录").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        if !self.config.model_dirs.contains(&folder) {
                            self.config.model_dirs.push(folder);
                            self.rescan();
                        }
                    }
                }
                if ui.button("重新扫描").clicked() {
                    self.rescan();
                }
            });

            ui.add_space(14.0);
            ui.heading("llama-server");
            let found = self
                .config
                .llama_server
                .clone()
                .filter(|p| p.is_file())
                .or_else(crate::config::find_llama_server);
            match &found {
                Some(path) => {
                    ui.label(RichText::new(format!("✓ {}", path.display())).color(GREEN));
                }
                None => {
                    ui.colored_label(RED, "未找到 llama-server：服务无法启动");
                }
            }
            ui.horizontal(|ui| {
                if ui.button("选择可执行文件").clicked() {
                    if let Some(file) = rfd::FileDialog::new().pick_file() {
                        self.config.llama_server = Some(file);
                        actions.push(Action::SaveConfig);
                    }
                }
                if ui.button("自动查找").clicked() {
                    self.config.llama_server = crate::config::find_llama_server();
                }
            });
            if let Some(path) = &self.config.llama_server {
                ui.label(RichText::new(path.display().to_string()).color(GREY).size(12.0));
            }
            ui.label(
                RichText::new("带动态库的 llama.cpp 发行包会自动设置 DYLD_LIBRARY_PATH。")
                    .color(GREY)
                    .size(12.0),
            );

            ui.add_space(14.0);
            ui.heading("刷新间隔");
            let mut secs = self.config.refresh_secs as u32;
            if ui
                .add(egui::Slider::new(&mut secs, 1..=30).suffix(" 秒"))
                .changed()
            {
                self.config.refresh_secs = secs as u64;
            }

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.button("保存配置").clicked() {
                    actions.push(Action::SaveConfig);
                }
                if ui.button("打开配置文件").clicked() {
                    actions.push(Action::Reveal(Config::path()));
                }
            });
            ui.add_space(10.0);
            ui.label(RichText::new(format!("配置: {}", Config::path().display())).color(GREY).size(12.0));
            ui.label(RichText::new(format!("状态: {}", StateStore::path().display())).color(GREY).size(12.0));
            ui.label(RichText::new(format!("日志: {}", runtime::logs_dir().display())).color(GREY).size(12.0));
        });
    }

    // ------------------------------------------------------------------ dialogs

    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.editor.take() else {
            return;
        };
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(if editor.original_id.is_some() { "编辑服务" } else { "新建服务" })
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                egui::Grid::new("editor_grid")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("名称");
                        ui.add(egui::TextEdit::singleline(&mut editor.name).desired_width(320.0));
                        ui.end_row();

                        ui.label("模型");
                        ui.horizontal(|ui| {
                            ui.label(util::truncate(&editor.model.display().to_string(), 44));
                            if ui.small_button("选择").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("GGUF", &["gguf"])
                                    .pick_file()
                                {
                                    editor.model = path;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("视觉投影");
                        ui.horizontal(|ui| {
                            match &editor.mmproj {
                                Some(path) => {
                                    ui.label(util::truncate(&path.display().to_string(), 36));
                                    if ui.small_button("清除").clicked() {
                                        editor.mmproj = None;
                                    }
                                }
                                None => {
                                    if ui.small_button("选择 mmproj").clicked() {
                                        if let Some(path) = rfd::FileDialog::new()
                                            .add_filter("GGUF", &["gguf"])
                                            .pick_file()
                                        {
                                            editor.mmproj = Some(path);
                                        }
                                    }
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("端口");
                        ui.add(egui::DragValue::new(&mut editor.port).range(1024..=65535));
                        ui.end_row();

                        ui.label("上下文");
                        ui.add(egui::DragValue::new(&mut editor.ctx).range(512..=131072).speed(256));
                        ui.end_row();

                        ui.label("GPU 层 (-ngl)");
                        ui.add(egui::DragValue::new(&mut editor.ngl).range(0..=999));
                        ui.end_row();

                        ui.label("并行槽 (-np)");
                        ui.add(egui::DragValue::new(&mut editor.parallel).range(1..=8));
                        ui.end_row();

                        ui.label("额外参数");
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.extra)
                                .hint_text("--chat-template ... 空格分隔")
                                .desired_width(320.0),
                        );
                        ui.end_row();
                    });
                ui.add_space(6.0);
                if let Some(server) = self
                    .config
                    .llama_server
                    .clone()
                    .or_else(crate::config::find_llama_server)
                {
                    let service = editor.to_service();
                    let command = service.command_line(&server).join(" ");
                    ui.collapsing("启动命令", |ui| {
                        ui.label(RichText::new(command).monospace().size(11.0));
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        submit = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            let service = editor.to_service();
            match &editor.original_id {
                Some(id) => {
                    if let Some(existing) = self.config.services.iter_mut().find(|s| &s.id == id) {
                        let mut updated = service;
                        updated.id = existing.id.clone();
                        *existing = updated;
                    }
                }
                None => self.config.add_service(service),
            }
            match self.config.save() {
                Ok(()) => self.toast("服务已保存", false),
                Err(err) => self.toast(format!("保存失败: {err}"), true),
            }
            self.poll();
        } else if !cancel && open {
            self.editor = Some(editor);
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(&confirm.title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(&confirm.body);
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for line in &confirm.detail {
                        ui.label(RichText::new(line).monospace().size(11.0).color(GREY));
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = if confirm.service.is_some() { "删除服务" } else { "移到废纸篓" };
                    if ui.button(RichText::new(label).color(RED)).clicked() {
                        confirmed = true;
                    }
                    if ui.button("取消").clicked() {
                        cancelled = true;
                    }
                });
            });
        if confirmed {
            if let Some(service) = confirm.service {
                actions.push(Action::RemoveService(service));
            } else {
                actions.push(Action::DeleteModel {
                    files: confirm.files,
                    size: confirm.size,
                });
            }
        } else if !cancelled && open {
            self.confirm = Some(confirm);
        }
    }

    fn toast_bar(&mut self, ctx: &egui::Context) {
        if let Some((message, error, at)) = self.toast.clone() {
            if at.elapsed() > Duration::from_secs(6) {
                self.toast = None;
                return;
            }
            egui::TopBottomPanel::bottom("toast").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(if error { RED } else { GREEN }, if error { "✕" } else { "✓" });
                    ui.label(message);
                });
            });
            ctx.request_repaint_after(Duration::from_millis(300));
        }
    }
}

impl Editor {
    fn from_service(service: &ServiceDef, _config: &Config) -> Self {
        Self {
            original_id: Some(service.id.clone()),
            name: service.name.clone(),
            model: service.model.clone(),
            mmproj: service.mmproj.clone(),
            port: service.port,
            ctx: service.ctx,
            ngl: service.ngl,
            parallel: service.parallel,
            image_min_tokens: service.image_min_tokens.unwrap_or(1024),
            extra: service.extra_args.join(" "),
        }
    }

    fn to_service(&self) -> ServiceDef {
        ServiceDef {
            id: self.original_id.clone().unwrap_or_else(|| crate::config::slug(&self.name)),
            name: if self.name.trim().is_empty() {
                "未命名服务".into()
            } else {
                self.name.trim().to_string()
            },
            model: self.model.clone(),
            mmproj: self.mmproj.clone(),
            host: "127.0.0.1".into(),
            port: self.port,
            ctx: self.ctx,
            ngl: self.ngl,
            parallel: self.parallel,
            image_min_tokens: self.mmproj.as_ref().map(|_| self.image_min_tokens),
            extra_args: self
                .extra
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut actions: Vec<Action> = Vec::new();

        if self.last_poll.elapsed() >= Duration::from_secs(self.config.refresh_secs.max(1)) {
            self.poll();
        }
        if !self.server_missing_warned {
            let ok = self
                .config
                .llama_server
                .as_ref()
                .map(|p| p.is_file())
                .unwrap_or(false)
                || crate::config::find_llama_server().is_some();
            if !ok {
                self.server_missing_warned = true;
            }
        }

        self.top_bar(ctx, &mut actions);
        self.nav(ctx);
        self.toast_bar(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                Tab::Models => self.models_tab(ui, &mut actions),
                Tab::Services => self.services_tab(ui, &mut actions),
                Tab::Status => self.status_tab(ui),
                Tab::Logs => self.logs_tab(ui),
                Tab::Settings => self.settings_tab(ui, &mut actions),
            }
        });

        self.editor_window(ctx);
        self.confirm_window(ctx, &mut actions);

        for action in actions {
            self.apply(action);
        }

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn parse_metrics(text: &str) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let key = name.split('{').next().unwrap_or(name).to_string();
        if let Some(value) = parts.next().and_then(|v| v.parse::<f64>().ok()) {
            out.insert(key, value);
        }
    }
    out
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
