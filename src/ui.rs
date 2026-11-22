//! egui desktop UI: page layout composed from the `theme` design kit.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Margin, Order, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Vec2,
};
use serde_json::Value;

use crate::config::{Config, ServiceDef};
use crate::hub::{self, HubFile, HubModel};
use crate::library::{self, ModelEntry};
use crate::runtime::{self, StateStore};
use crate::theme::{self, *};
use crate::util;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Discover,
    Models,
    Services,
    Status,
    Logs,
    Settings,
}

const NAV: [(Tab, &str, NavIcon); 6] = [
    (Tab::Discover, "发现模型", NavIcon::Discover),
    (Tab::Models, "模型库", NavIcon::Models),
    (Tab::Services, "服务", NavIcon::Services),
    (Tab::Status, "运行状态", NavIcon::Status),
    (Tab::Logs, "日志", NavIcon::Logs),
    (Tab::Settings, "设置", NavIcon::Settings),
];

fn page_meta(tab: Tab) -> (&'static str, &'static str) {
    match tab {
        Tab::Discover => (
            "发现模型",
            "从 Hugging Face 检索 GGUF 模型，下载后即可直接启动",
        ),
        Tab::Models => ("模型库", "扫描本地 GGUF 模型，一键创建推理服务"),
        Tab::Services => ("服务", "管理 llama-server 进程、端口与启动参数"),
        Tab::Status => ("运行状态", "实时健康检查、吞吐与资源占用"),
        Tab::Logs => ("日志", "跟随服务输出，排查启动与推理问题"),
        Tab::Settings => ("设置", "模型目录、可执行文件与刷新偏好"),
    }
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

impl Status {
    fn healthy(&self) -> bool {
        self.health
            .as_ref()
            .and_then(|h| h.get("status"))
            .and_then(|v| v.as_str())
            == Some("ok")
    }

    fn tokens(&self) -> f64 {
        self.metrics
            .get("llamacpp:tokens_predicted_total")
            .copied()
            .unwrap_or(0.0)
    }

    fn prompt_tokens(&self) -> f64 {
        self.metrics
            .get("llamacpp:tokens_evaluated_total")
            .or_else(|| self.metrics.get("llamacpp:prompt_tokens_total"))
            .copied()
            .unwrap_or(0.0)
    }

    fn kv(&self) -> Option<f64> {
        self.metrics.get("llamacpp:kv_cache_usage_ratio").copied()
    }

    fn n_ctx(&self) -> Option<u64> {
        self.props
            .as_ref()
            .and_then(|p| p.get("default_generation_settings"))
            .and_then(|v| v.get("n_ctx"))
            .and_then(|v| v.as_u64())
    }

    fn slots(&self) -> Option<u64> {
        self.props
            .as_ref()
            .and_then(|p| p.get("total_slots"))
            .and_then(|v| v.as_u64())
    }
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
    HubSearch,
    HubOpenFiles(String),
    HubInstall { model: String, file: HubFile },
    HubAutoInstall(String),
    HubCancel(String),
    HubForget(String),
}

struct PickerState {
    model: String,
    loading: bool,
    error: Option<String>,
}

#[derive(Default)]
struct HubState {
    query: String,
    results: Vec<HubModel>,
    loading: bool,
    error: Option<String>,
    /// Endpoint that answered the last request.
    base: Option<String>,
    searched: bool,
    files: HashMap<String, Vec<HubFile>>,
    files_rx: Option<(String, Receiver<hub::FetchResult>)>,
    /// 批量拉取文件清单（列表里显示仓库总大小），带批次号防止旧结果覆盖新搜索。
    listing_rx: Option<hub::ListingRx>,
    listing_gen: u64,
    search_rx: Option<Receiver<hub::FetchResult>>,
    /// Model whose recommended file should start downloading as soon as the
    /// file list arrives.
    auto_install: Option<String>,
    picker: Option<PickerState>,
    downloads: Vec<hub::Download>,
    last_tick: Option<Instant>,
    installed: HashSet<String>,
    /// Forces the result list back to the top on the next frame.
    scroll_top: bool,
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
    hub: HubState,
    placed: bool,
}

pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 860.0])
            .with_min_inner_size([980.0, 640.0])
            .with_title("LLM Manager — 本地大模型管理"),
        ..Default::default()
    };
    eframe::run_native(
        "LLM Manager",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

impl App {
    fn new() -> Self {
        let config = Config::load();
        let mut state = StateStore::load();
        state.prune();
        let (models, scan_errors) = library::scan(&config.model_dirs);
        let tab = match std::env::var("LLMMGR_TAB").as_deref() {
            Ok("discover") => Tab::Discover,
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
            hub: HubState::default(),
            placed: false,
        };
        app.poll();
        app
    }

    fn toast(&mut self, message: impl Into<String>, error: bool) {
        self.toast = Some((message.into(), error, Instant::now()));
    }

    fn running_count(&self) -> usize {
        self.status.values().filter(|s| s.running).count()
    }

    fn running_paths(&self) -> HashSet<PathBuf> {
        self.config
            .services
            .iter()
            .filter(|s| self.status.get(&s.id).map(|st| st.running).unwrap_or(false))
            .map(|s| s.model.clone())
            .collect()
    }

    fn server_path(&self) -> Option<PathBuf> {
        self.config
            .llama_server
            .clone()
            .filter(|p| p.is_file())
            .or_else(crate::config::find_llama_server)
    }

    /// Refresh process/health state for every configured service.
    fn poll(&mut self) {
        self.last_poll = Instant::now();
        self.sys = runtime::sys_info();
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
        self.reload_log();
    }

    fn reload_log(&mut self) {
        let Some(id) = self.log_for.clone() else {
            return;
        };
        if let Some(run) = self.state.services.get(&id) {
            if runtime::is_alive(run.pid) || run.log.is_file() {
                self.log_lines = runtime::log_tail(&run.log, 400);
            }
        }
    }

    fn ensure_log(&mut self) {
        if self.log_for.is_none() {
            self.log_for = self.config.services.first().map(|s| s.id.clone());
        }
        self.reload_log();
    }

    fn rescan(&mut self) {
        let (models, errors) = library::scan(&self.config.model_dirs);
        self.models = models;
        self.scan_errors = errors;
    }

    // ---------------------------------------------------------------- hub

    fn hub_bases(&self) -> Vec<String> {
        hub::candidates(self.config.hub_endpoint.as_deref())
    }

    fn hub_source_label(&self) -> String {
        let endpoint = self
            .hub
            .base
            .clone()
            .or_else(|| self.hub_bases().into_iter().next())
            .unwrap_or_default();
        let host = endpoint
            .split("://")
            .nth(1)
            .unwrap_or(&endpoint)
            .split('/')
            .next()
            .unwrap_or(&endpoint)
            .strip_prefix("www.")
            .map(str::to_string)
            .unwrap_or_else(|| endpoint.clone());
        host
    }

    fn start_hub_search(&mut self) {
        if self.hub.search_rx.is_some() {
            return;
        }
        self.hub.loading = true;
        self.hub.searched = true;
        self.hub.error = None;
        self.hub.scroll_top = true;
        let bases = self.hub_bases();
        self.hub.search_rx = Some(hub::spawn(
            hub::FetchKind::Search,
            bases,
            self.hub.query.clone(),
        ));
    }

    fn start_files_fetch(&mut self, model: &str) {
        if self.hub.files_rx.is_some() {
            return;
        }
        let bases = self.hub_bases();
        self.hub.files_rx = Some((
            model.to_string(),
            hub::spawn(hub::FetchKind::Files, bases, model.to_string()),
        ));
    }

    /// 搜索完成后后台批量读取每个仓库的文件清单，用来显示总大小。
    fn start_listing(&mut self) {
        let Some(base) = self.hub.base.clone() else {
            return;
        };
        let ids: Vec<String> = self.hub.results.iter().map(|m| m.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        self.hub.listing_gen += 1;
        self.hub.listing_rx = Some(hub::spawn_listing(base, ids, self.hub.listing_gen));
    }

    fn local_file(&self, name: &str) -> Option<PathBuf> {
        for dir in &self.config.model_dirs {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }

    fn download_dir(&mut self) -> Option<PathBuf> {
        match self.config.model_dirs.first().cloned() {
            Some(dir) => Some(dir),
            None => {
                self.toast("请先在设置里添加一个模型目录", true);
                self.tab = Tab::Settings;
                None
            }
        }
    }

    fn start_download(&mut self, model: &str, file: &HubFile) {
        let Some(dir) = self.download_dir() else {
            return;
        };
        if self
            .hub
            .downloads
            .iter()
            .any(|d| d.file == file.name && !d.done)
        {
            return;
        }
        match hub::preflight(&dir, &file.name, file.size) {
            Ok(None) => {}
            Ok(Some(warning)) => self.toast(warning, true),
            Err(err) => {
                self.toast(err, true);
                return;
            }
        }
        let base = self
            .hub
            .base
            .clone()
            .or_else(|| self.hub_bases().into_iter().next())
            .unwrap_or_else(|| hub::MIRROR.to_string());
        match hub::Download::start(&base, model, &file.name, &dir, file.size) {
            Ok(download) => {
                self.hub.downloads.push(download);
                self.hub.last_tick = Some(Instant::now());
            }
            Err(err) => {
                if err == "文件已存在" {
                    self.hub.installed.insert(model.to_string());
                    self.toast("这个文件已经在模型目录里了", false);
                } else {
                    self.toast(format!("无法开始下载: {err}"), true);
                }
            }
        }
    }

    /// Drain background requests and sample download progress. Runs every frame.
    fn hub_poll(&mut self) {
        let mut listing: Vec<(u64, String, Result<Vec<HubFile>, String>)> = Vec::new();
        let mut listing_done = false;
        if let Some(rx) = &self.hub.listing_rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => listing.push(item),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        listing_done = true;
                        break;
                    }
                }
            }
        }
        if listing_done {
            self.hub.listing_rx = None;
        }
        let gen = self.hub.listing_gen;
        for (batch, id, files) in listing {
            if batch == gen {
                if let Ok(files) = files {
                    self.hub.files.insert(id, files);
                }
            }
        }

        if let Some(rx) = self.hub.search_rx.take() {
            match rx.try_recv() {
                Ok(result) => {
                    self.hub.loading = false;
                    match result.body {
                        Ok(body) => match hub::parse_search(&body) {
                            Ok(list) => {
                                self.hub.base = Some(result.base);
                                self.hub.results = list;
                                self.hub.error = None;
                                self.start_listing();
                            }
                            Err(err) => self.hub.error = Some(err),
                        },
                        Err(err) => {
                            self.hub.error = Some(format!(
                                "检索失败：{err}。可在「设置 → 模型下载源」里切换站点。"
                            ))
                        }
                    }
                }
                Err(TryRecvError::Empty) => self.hub.search_rx = Some(rx),
                Err(TryRecvError::Disconnected) => {
                    self.hub.loading = false;
                    self.hub.error = Some("检索线程意外退出".into());
                }
            }
        }

        if let Some((model, rx)) = self.hub.files_rx.take() {
            match rx.try_recv() {
                Ok(result) => {
                    let outcome = result
                        .body
                        .and_then(|body| hub::parse_files(&body).map(|files| (result.base, files)));
                    match outcome {
                        Ok((base, files)) => {
                            self.hub.base = Some(base);
                            self.hub.files.insert(model.clone(), files.clone());
                            if let Some(picker) = self.hub.picker.as_mut() {
                                if picker.model == model {
                                    picker.loading = false;
                                    picker.error = None;
                                }
                            }
                            if self.hub.auto_install.as_deref() == Some(model.as_str()) {
                                self.hub.auto_install = None;
                                if let Some(idx) = hub::recommend(&files) {
                                    self.start_download(&model, &files[idx]);
                                }
                            }
                        }
                        Err(err) => {
                            let message = format!("读取文件列表失败：{err}");
                            if let Some(picker) = self.hub.picker.as_mut() {
                                if picker.model == model {
                                    picker.loading = false;
                                    picker.error = Some(message.clone());
                                }
                            } else {
                                self.toast(message, true);
                            }
                            self.hub.auto_install = None;
                        }
                    }
                }
                Err(TryRecvError::Empty) => self.hub.files_rx = Some((model, rx)),
                Err(TryRecvError::Disconnected) => {
                    self.hub.files_rx = None;
                    self.hub.auto_install = None;
                }
            }
        }

        let due = self
            .hub
            .last_tick
            .map(|tick| tick.elapsed() >= Duration::from_millis(250))
            .unwrap_or(false);
        if !due || self.hub.downloads.is_empty() {
            return;
        }
        self.hub.last_tick = Some(Instant::now());
        let mut finished: Vec<(bool, String, String)> = Vec::new();
        for download in &mut self.hub.downloads {
            if download.poll() {
                let ok = download.error.is_none();
                let detail = download
                    .error
                    .clone()
                    .unwrap_or_else(|| download.file.clone());
                finished.push((ok, download.model.clone(), detail));
            }
        }
        for (ok, model, detail) in finished {
            if ok {
                self.hub.installed.insert(model);
                self.rescan();
                self.toast(format!("已安装 {detail}"), false);
            } else {
                self.toast(detail.to_string(), true);
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Start(id) => {
                if let Some(service) = self.config.service(&id).cloned() {
                    match runtime::start(&self.config, &service, &mut self.state) {
                        Ok(pid) => {
                            self.toast(format!("已启动 {} (pid {pid})", service.name), false)
                        }
                        Err(err) => self.toast(format!("启动失败: {err}"), true),
                    }
                }
            }
            Action::Stop(id) => {
                let name = self
                    .config
                    .service(&id)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
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
            Action::DeleteModel { files, size } => match library::delete_files(&files) {
                Ok(freed) => {
                    self.toast(
                        format!(
                            "已删除 {} 个文件，释放 {}",
                            files.len(),
                            util::human_bytes(freed)
                        ),
                        false,
                    );
                    self.rescan();
                }
                Err(err) => self.toast(
                    format!("删除失败: {err} ({} 释放)", util::human_bytes(size)),
                    true,
                ),
            },
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
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn();
            }
            Action::HubSearch => self.start_hub_search(),
            Action::HubOpenFiles(model) => {
                let cached = self.hub.files.contains_key(&model);
                self.hub.picker = Some(PickerState {
                    model: model.clone(),
                    loading: !cached,
                    error: None,
                });
                if !cached {
                    self.start_files_fetch(&model);
                }
            }
            Action::HubInstall { model, file } => self.start_download(&model, &file),
            Action::HubAutoInstall(model) => match self.hub.files.get(&model).cloned() {
                Some(files) => match hub::recommend(&files) {
                    Some(idx) => {
                        let file = files[idx].clone();
                        self.start_download(&model, &file);
                    }
                    None => self.toast("这个仓库没有可下载的 GGUF 文件", true),
                },
                None => {
                    self.hub.auto_install = Some(model.clone());
                    self.start_files_fetch(&model);
                }
            },
            Action::HubCancel(key) => {
                for download in self.hub.downloads.iter_mut() {
                    if download.key() == key {
                        download.cancel();
                    }
                }
            }
            Action::HubForget(key) => {
                self.hub.downloads.retain(|d| d.key() != key);
            }
        }
    }

    // ------------------------------------------------------------- chrome

    fn top_bar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        egui::TopBottomPanel::top("top")
            .exact_height(74.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin {
                left: 24,
                right: 24,
                top: 0,
                bottom: 0,
            }))
            .show(ctx, |ui| {
                let (title, subtitle) = page_meta(self.tab);
                ui.add_space(13.0);
                ui.horizontal(|ui| {
                    let galley = ui.painter().layout_no_wrap(
                        title.to_owned(),
                        FontId::proportional(18.5),
                        TEXT,
                    );
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(galley.size().x.max(1.0), 30.0),
                        Sense::hover(),
                    );
                    if ui.is_rect_visible(rect) {
                        ui.painter().galley(
                            Pos2::new(rect.left(), rect.center().y - galley.size().y * 0.5 - 1.0),
                            galley,
                            TEXT,
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "刷新", Kind::Ghost).clicked() {
                            actions.push(Action::Rescan);
                        }
                        ui.add_space(6.0);
                        let total: u64 = self.models.iter().map(|m| m.total_size()).sum();
                        chip(
                            ui,
                            &format!(
                                "{} 个模型 · {}",
                                self.models.len(),
                                util::human_bytes(total)
                            ),
                            None,
                        );
                        ui.add_space(2.0);
                        let running = self.running_count();
                        chip(
                            ui,
                            &format!("{running} 个服务运行中"),
                            Some(if running > 0 { GREEN } else { FAINT }),
                        );
                    });
                });
                ui.add_space(1.0);
                ui.label(RichText::new(subtitle).size(11.0).color(FAINT));
                let rect = ui.max_rect();
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    Stroke::new(1.0_f32, BORDER),
                );
            });
    }

    fn nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .exact_width(212.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(14, 18)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(10), ACCENT);
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "λ",
                        FontId::proportional(20.0),
                        Color32::WHITE,
                    );
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("LLM Manager").size(14.0).strong().color(TEXT));
                        ui.label(RichText::new("本地大模型管理").size(10.5).color(FAINT));
                    });
                });
                ui.add_space(24.0);

                for (tab, label, icon) in NAV {
                    if nav_item(ui, label, icon, self.tab == tab).clicked() {
                        self.tab = tab;
                        if tab == Tab::Logs {
                            self.ensure_log();
                        }
                    }
                    ui.add_space(2.0);
                }

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(10.0)
                            .color(FAINT),
                    );
                    ui.add_space(10.0);
                    self.server_footer(ui);
                });
            });
    }

    fn server_footer(&self, ui: &mut egui::Ui) {
        let path = self.server_path();
        let found = path.is_some();
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(10)
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    status_dot(ui, if found { GREEN } else { AMBER }, found);
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(if found {
                            "llama-server 就绪"
                        } else {
                            "未找到 llama-server"
                        })
                        .size(11.5)
                        .color(if found { TEXT_DIM } else { AMBER }),
                    );
                });
                ui.add_space(3.0);
                match &path {
                    Some(path) => {
                        let full = path.display().to_string();
                        let short = full.strip_prefix("/Users/").unwrap_or(&full).to_string();
                        ui.add(
                            egui::Label::new(RichText::new(short).size(10.0).color(FAINT))
                                .truncate(),
                        )
                        .on_hover_text(full);
                    }
                    None => {
                        ui.label(
                            RichText::new("在设置中指定可执行文件")
                                .size(10.0)
                                .color(FAINT),
                        );
                    }
                }
            });
    }

    // -------------------------------------------------------------- pages

    fn discover_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.hub.query)
                    .hint_text("搜索模型，例如 qwen2.5 7b、llama 3.1、deepseek r1")
                    .desired_width(360.0)
                    .margin(Margin::symmetric(10, 7))
                    .vertical_align(Align::Center),
            );
            let entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (button(ui, "搜索", Kind::Primary).clicked() || entered) && !self.hub.loading {
                actions.push(Action::HubSearch);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "刷新", Kind::Ghost).clicked() && !self.hub.loading {
                    actions.push(Action::HubSearch);
                }
                ui.add_space(6.0);
                let label = self.hub_source_label();
                let tone = if self.hub.base.is_some() { TEAL } else { FAINT };
                chip(ui, &label, Some(tone));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if self.hub.loading {
                        "正在检索…".to_string()
                    } else if self.hub.searched {
                        format!("{} 个结果", self.hub.results.len())
                    } else {
                        "热门模型".to_string()
                    })
                    .size(11.5)
                    .color(FAINT),
                );
            });
        });
        ui.add_space(14.0);

        if !self.hub.downloads.is_empty() {
            let downloads = std::mem::take(&mut self.hub.downloads);
            for download in &downloads {
                hub_download_card(ui, download, actions);
                ui.add_space(8.0);
            }
            self.hub.downloads = downloads;
        }

        if let Some(err) = self.hub.error.clone() {
            egui::Frame::new()
                .fill(alpha(RED, 14))
                .stroke(Stroke::new(1.0_f32, alpha(RED, 56)))
                .corner_radius(10)
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("检索失败").size(11.5).strong().color(RED));
                        ui.label(RichText::new(err).size(11.5).color(TEXT_DIM));
                    });
                });
            ui.add_space(10.0);
        }

        if self.hub.loading && self.hub.results.is_empty() {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "正在从 {} 检索 GGUF 模型…",
                            self.hub_source_label()
                        ))
                        .size(12.0)
                        .color(TEXT_DIM),
                    );
                });
            });
            return;
        }

        if self.hub.results.is_empty() {
            if empty_state(
                ui,
                NavIcon::Discover,
                "搜索并安装 GGUF 模型",
                "输入关键词检索 Hugging Face 上的 GGUF 仓库，挑一个量化文件下载到本地模型目录。",
                Some("看看热门模型"),
            ) {
                self.hub.query.clear();
                actions.push(Action::HubSearch);
            }
            return;
        }

        let free = self
            .config
            .model_dirs
            .first()
            .and_then(|dir| hub::free_space(dir));
        let results = std::mem::take(&mut self.hub.results);
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if self.hub.scroll_top {
            area = area.vertical_scroll_offset(0.0);
            self.hub.scroll_top = false;
        }
        area.show(ui, |ui| {
            // The scroll bar is drawn on top of the content: keep the cards
            // clear of it so nothing gets clipped at the right edge.
            let reserved =
                ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin * 2.0 + 4.0;
            ui.set_width(ui.available_width() - reserved);
            ui.spacing_mut().item_spacing.y = 8.0;
            for model in &results {
                let installed_here = self.hub.installed.contains(&model.id)
                    || self
                        .hub
                        .files
                        .get(&model.id)
                        .map(|files| {
                            files
                                .iter()
                                .any(|file| self.local_file(&file.name).is_some())
                        })
                        .unwrap_or(false);
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let right_w = 250.0;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let info_w = (ui.available_width() - right_w).max(220.0);
                        region(
                            ui,
                            Vec2::new(info_w, 44.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.spacing_mut().item_spacing.y = 5.0;
                                let mut title = egui::text::LayoutJob::default();
                                title.wrap.max_width = info_w - 16.0;
                                title.wrap.max_rows = 1;
                                title.append(
                                    model.owner(),
                                    0.0,
                                    egui::TextFormat {
                                        font_id: FontId::proportional(12.5),
                                        color: MUTED,
                                        ..Default::default()
                                    },
                                );
                                title.append(
                                    " / ",
                                    0.0,
                                    egui::TextFormat {
                                        font_id: FontId::proportional(12.5),
                                        color: FAINT,
                                        ..Default::default()
                                    },
                                );
                                title.append(
                                    &util::truncate(model.name(), 46),
                                    0.0,
                                    egui::TextFormat {
                                        font_id: FontId::proportional(14.0),
                                        color: TEXT,
                                        ..Default::default()
                                    },
                                );
                                ui.label(title);

                                let mut meta = egui::text::LayoutJob::default();
                                meta.wrap.max_width = info_w - 16.0;
                                meta.wrap.max_rows = 1;
                                meta.append(
                                    &format!(
                                        "下载 {} · 赞 {}",
                                        count_label(model.downloads),
                                        count_label(model.likes)
                                    ),
                                    0.0,
                                    egui::TextFormat {
                                        font_id: FontId::proportional(11.0),
                                        color: FAINT,
                                        ..Default::default()
                                    },
                                );
                                let files = self.hub.files.get(&model.id);
                                let recommended = files
                                    .and_then(|list| hub::recommend(list).map(|i| list[i].clone()));
                                match &recommended {
                                    Some(file) => {
                                        let quant = hub::quant_of(&file.name);
                                        let label = if quant == "?" {
                                            format!("推荐 {}", util::human_bytes(file.size))
                                        } else {
                                            format!("推荐 {quant} {}", util::human_bytes(file.size))
                                        };
                                        meta.append(
                                            &format!(
                                                "  ·  {} 个文件 · {label}",
                                                files.map(|f| f.len()).unwrap_or(0)
                                            ),
                                            0.0,
                                            egui::TextFormat {
                                                font_id: FontId::proportional(11.0),
                                                color: TEXT_DIM,
                                                ..Default::default()
                                            },
                                        );
                                        if let Some(free) = free {
                                            if file.size + 64 * 1024 * 1024 > free {
                                                meta.append(
                                                    "  ·  空间不足",
                                                    0.0,
                                                    egui::TextFormat {
                                                        font_id: FontId::proportional(11.0),
                                                        color: RED,
                                                        ..Default::default()
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    None => match files {
                                        Some(list) => {
                                            let total: u64 = list.iter().map(|f| f.size).sum();
                                            meta.append(
                                                &format!(
                                                    "  ·  {} 个文件 · 共 {}",
                                                    list.len(),
                                                    util::human_bytes(total)
                                                ),
                                                0.0,
                                                egui::TextFormat {
                                                    font_id: FontId::proportional(11.0),
                                                    color: TEXT_DIM,
                                                    ..Default::default()
                                                },
                                            );
                                        }
                                        None => meta.append(
                                            "  ·  计算体积…",
                                            0.0,
                                            egui::TextFormat {
                                                font_id: FontId::proportional(11.0),
                                                color: FAINT,
                                                ..Default::default()
                                            },
                                        ),
                                    },
                                }
                                if installed_here {
                                    meta.append(
                                        "  ·  已安装",
                                        0.0,
                                        egui::TextFormat {
                                            font_id: FontId::proportional(11.0),
                                            color: GREEN,
                                            ..Default::default()
                                        },
                                    );
                                }
                                let response = ui.label(meta);
                                if let Some(list) = files {
                                    let total: u64 = list.iter().map(|f| f.size).sum();
                                    response.on_hover_text(format!(
                                        "仓库共 {} 个 GGUF 文件，合计 {}",
                                        list.len(),
                                        util::human_bytes(total)
                                    ));
                                }
                            },
                        );
                        region(
                            ui,
                            Vec2::new(right_w, 44.0),
                            Layout::right_to_left(Align::Center),
                            |ui| {
                                if installed_here {
                                    if button(ui, "选择文件", Kind::Ghost).clicked() {
                                        actions.push(Action::HubOpenFiles(model.id.clone()));
                                    }
                                } else {
                                    if button(ui, "一键安装", Kind::Primary).clicked() {
                                        actions.push(Action::HubAutoInstall(model.id.clone()));
                                    }
                                    ui.add_space(6.0);
                                    if button(ui, "选择文件", Kind::Ghost).clicked() {
                                        actions.push(Action::HubOpenFiles(model.id.clone()));
                                    }
                                }
                            },
                        );
                    });
                });
            }
        });
        self.hub.results = results;
    }

    fn models_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let total: u64 = self.models.iter().map(|m| m.total_size()).sum();
        let running = self.running_paths();

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("搜索模型 / 量化 / 架构")
                    .desired_width(300.0)
                    .margin(Margin::symmetric(10, 7))
                    .vertical_align(Align::Center),
            );
            if !self.filter.is_empty() && icon_button(ui, "✕", "清除搜索", Kind::Quiet).clicked()
            {
                self.filter.clear();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "打开模型目录", Kind::Ghost).clicked() {
                    if let Some(dir) = self.config.model_dirs.first() {
                        actions.push(Action::Reveal(dir.clone()));
                    }
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} 个模型 · {}",
                        self.models.len(),
                        util::human_bytes(total)
                    ))
                    .size(11.5)
                    .color(FAINT),
                );
            });
        });
        ui.add_space(14.0);

        if !self.scan_errors.is_empty() {
            let first = self.scan_errors.first().cloned().unwrap_or_default();
            let more = self.scan_errors.len().saturating_sub(1);
            egui::Frame::new()
                .fill(alpha(AMBER, 16))
                .stroke(Stroke::new(1.0_f32, alpha(AMBER, 60)))
                .corner_radius(10)
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("扫描警告").size(11.5).strong().color(AMBER));
                        ui.label(
                            RichText::new(if more > 0 {
                                format!("{first}（另有 {more} 条）")
                            } else {
                                first
                            })
                            .size(11.5)
                            .color(TEXT_DIM),
                        );
                    });
                });
            ui.add_space(10.0);
        }

        if self.config.model_dirs.is_empty() {
            if empty_state(
                ui,
                NavIcon::Models,
                "还没有添加模型目录",
                "指定一个存放 .gguf 文件的目录，LLM Manager 会自动扫描并识别量化与上下文。",
                Some("前往设置"),
            ) {
                self.tab = Tab::Settings;
            }
            return;
        }

        let filter = self.filter.to_lowercase();
        let models: Vec<ModelEntry> = self
            .models
            .iter()
            .filter(|m| {
                filter.is_empty()
                    || m.file_name.to_lowercase().contains(&filter)
                    || m.quant().to_lowercase().contains(&filter)
                    || m.arch().to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();

        if models.is_empty() {
            if self.models.is_empty() {
                if empty_state(
                    ui,
                    NavIcon::Models,
                    "目录里没有模型",
                    "当前模型目录没有找到 .gguf 文件，确认路径后重新扫描。",
                    Some("前往设置"),
                ) {
                    self.tab = Tab::Settings;
                }
            } else {
                empty_state(
                    ui,
                    NavIcon::Models,
                    "没有匹配的模型",
                    "试试其他的名称、量化或架构关键词。",
                    None,
                );
            }
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for model in &models {
                    let is_running = running.contains(&model.path);
                    let (response, _) = hover_card(ui, |ui| {
                        let actions_w = 268.0;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let info_w = (ui.available_width() - actions_w).max(280.0);

                            region(ui, Vec2::new(18.0, 48.0), Layout::top_down(Align::Min), |ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::new(18.0, 48.0), Sense::hover());
                                if ui.is_rect_visible(rect) {
                                    dot(
                                        ui.painter(),
                                        Pos2::new(rect.left() + 4.0, rect.center().y),
                                        if is_running { GREEN } else { FAINT },
                                        is_running,
                                    );
                                }
                            });

                            region(
                                ui,
                                Vec2::new(info_w, 48.0),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(
                                                util::truncate(&model.display_name(), 46),
                                            )
                                            .size(14.5)
                                            .strong()
                                            .color(TEXT),
                                        );
                                        if model.files.len() > 1 {
                                            badge(
                                                ui,
                                                &format!("{} 分片", model.files.len()),
                                                MUTED,
                                            );
                                        }
                                        if model.mmproj.is_some() {
                                            badge(ui, "视觉", TEAL);
                                        }
                                        if model.meta.is_none() && model.error.is_some() {
                                            badge(ui, "元数据异常", AMBER);
                                        }
                                    });
                                    ui.add_space(7.0);
                                    ui.horizontal(|ui| {
                                        let quant = model.quant();
                                        if !quant.is_empty() {
                                            badge(ui, &quant, quant_tone(&quant));
                                        }
                                        let mut meta_line = format!(
                                            "{} · {} · {}",
                                            util::human_bytes(model.total_size()),
                                            model.arch(),
                                            model.params()
                                        );
                                        if let Some(meta) = &model.meta {
                                            if meta.context_length > 0 {
                                                meta_line.push_str(&format!(
                                                    " · ctx {}",
                                                    meta.context_length
                                                ));
                                            }
                                        }
                                        if let Some(ts) = model.modified {
                                            let ago = util::duration_label(
                                                util::now_unix().saturating_sub(ts),
                                            );
                                            meta_line.push_str(&format!(" · {ago}前"));
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(meta_line).size(11.5).color(FAINT),
                                            )
                                            .truncate(),
                                        );
                                    });
                                },
                            );

                            region(
                                ui,
                                Vec2::new(actions_w, 48.0),
                                Layout::right_to_left(Align::Center),
                                |ui| {
                                    if button(ui, "删除", Kind::Danger).clicked() {
                                        let mut detail = Vec::new();
                                        for file in model.all_files() {
                                            detail.push(file.display().to_string());
                                        }
                                        self.confirm = Some(Confirm {
                                            title: "删除模型".into(),
                                            body: format!(
                                                "确定删除 {} 及其 {} 个文件？共 {}，该操作不可撤销。",
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
                                    ui.add_space(4.0);
                                    if button(ui, "访达", Kind::Quiet).clicked() {
                                        actions.push(Action::Reveal(model.path.clone()));
                                    }
                                    ui.add_space(4.0);
                                    let label = if is_running { "新建服务" } else { "启动" };
                                    let kind = if is_running { Kind::Ghost } else { Kind::Primary };
                                    if button(ui, label, kind).clicked() {
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
                                },
                            );
                        });
                    });
                    response.on_hover_text(model_tooltip(model));
                    ui.add_space(8.0);
                }
            });
    }

    fn services_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let services = self.config.services.clone();
        let running = services
            .iter()
            .filter(|s| self.status.get(&s.id).map(|st| st.running).unwrap_or(false))
            .count();

        ui.horizontal(|ui| {
            if button(ui, "+  新建服务", Kind::Primary).clicked() {
                let port = self.config.next_free_port();
                let model = self
                    .models
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default();
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
            if running > 0 && button(ui, "全部停止", Kind::Ghost).clicked() {
                for service in &services {
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
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{running}/{} 运行中", services.len()))
                        .size(11.5)
                        .color(if running > 0 { GREEN } else { FAINT }),
                );
            });
        });
        ui.add_space(14.0);

        if services.is_empty() {
            if empty_state(
                ui,
                NavIcon::Services,
                "还没有服务",
                "从模型库选一个模型启动，或者在这里手动新建一份配置。",
                Some("新建服务"),
            ) {
                let port = self.config.next_free_port();
                let model = self
                    .models
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default();
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
            return;
        }

        let available = ui.available_width();
        let gap = 12.0;
        let two_columns = available > 900.0;
        let card_width = if two_columns {
            (available - gap) / 2.0
        } else {
            available
        };
        let per_row = if two_columns { 2 } else { 1 };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for chunk in services.chunks(per_row) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for service in chunk {
                            theme::column(ui, card_width, |ui| {
                                self.service_card(ui, service, actions)
                            });
                            ui.add_space(gap);
                        }
                    });
                    ui.add_space(gap);
                }
            });
    }

    fn service_card(&mut self, ui: &mut egui::Ui, service: &ServiceDef, actions: &mut Vec<Action>) {
        let status = self.status.get(&service.id).cloned().unwrap_or_default();
        let tone = if status.running { GREEN } else { FAINT };
        let label = if status.running {
            if status.healthy() {
                "运行中"
            } else {
                "启动中"
            }
        } else {
            "已停止"
        };
        let tone = if status.running && !status.healthy() {
            AMBER
        } else {
            tone
        };
        let id = service.id.clone();

        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&service.name).size(14.5).strong().color(TEXT));
                if service.mmproj.is_some() {
                    badge(ui, "视觉", TEAL);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    pill(ui, label, tone);
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("{}:{}", service.host, service.port))
                    .monospace()
                    .size(11.5)
                    .color(ACCENT_TEXT),
            );
            ui.add_space(6.0);
            let model_name = service
                .model
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "未选择模型".into());
            ui.label(
                RichText::new(util::truncate(&model_name, 56))
                    .size(11.0)
                    .color(FAINT),
            )
            .on_hover_text(service.model.display().to_string());
            ui.add_space(10.0);
            divider(ui);

            if status.running {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    dot_label(ui, FAINT, &format!("pid {}", status.pid));
                    dot_label(ui, FAINT, &util::duration_label(status.uptime));
                    dot_label(
                        ui,
                        FAINT,
                        &status
                            .rss
                            .map(util::human_bytes)
                            .unwrap_or_else(|| "—".into()),
                    );
                    dot_label(ui, FAINT, &format!("{:.0} tokens", status.tokens()));
                });
                ui.add_space(6.0);
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    dot_label(ui, FAINT, "未运行");
                    dot_label(ui, FAINT, &format!("ctx {}", service.ctx));
                    dot_label(
                        ui,
                        FAINT,
                        &if service.parallel > 1 {
                            format!("{} 并行槽", service.parallel)
                        } else {
                            "单槽".to_string()
                        },
                    );
                });
                ui.add_space(6.0);
            }
            if let Some(err) = &status.error {
                ui.label(
                    RichText::new(format!("健康检查失败：{err}"))
                        .size(11.0)
                        .color(AMBER),
                );
                ui.add_space(6.0);
            }

            ui.horizontal(|ui| {
                if status.running {
                    if button(ui, "停止", Kind::Ghost).clicked() {
                        actions.push(Action::Stop(id.clone()));
                    }
                    if button(ui, "重启", Kind::Ghost).clicked() {
                        actions.push(Action::Restart(id.clone()));
                    }
                } else if button(ui, "启动", Kind::Primary).clicked() {
                    actions.push(Action::Start(id.clone()));
                }
                if button(ui, "日志", Kind::Quiet).clicked() {
                    self.tab = Tab::Logs;
                    self.log_for = Some(id.clone());
                    self.log_lines.clear();
                    self.reload_log();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button(ui, "删除", Kind::Danger).clicked() {
                        self.confirm = Some(Confirm {
                            title: "删除服务".into(),
                            body: format!("确定删除服务「{}」？模型文件不会被删除。", service.name),
                            detail: Vec::new(),
                            files: Vec::new(),
                            size: 0,
                            service: Some(id.clone()),
                        });
                    }
                    if button(ui, "编辑", Kind::Quiet).clicked() {
                        self.editor = Some(Editor::from_service(service, &self.config));
                    }
                });
            });
        });
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if button(ui, "立即刷新", Kind::Ghost).clicked() {
                self.last_poll = Instant::now() - Duration::from_secs(60);
                self.poll();
            }
            ui.label(
                RichText::new(format!(
                    "每 {} 秒自动轮询 /health · /props · /metrics",
                    self.config.refresh_secs.max(1)
                ))
                .size(11.5)
                .color(FAINT),
            );
        });
        ui.add_space(14.0);

        let used = self.sys.total_ram.saturating_sub(self.sys.free_ram);
        let ram_ratio = if self.sys.total_ram > 0 {
            used as f32 / self.sys.total_ram as f32
        } else {
            0.0
        };
        let disk_ratio = if self.sys.disk_total > 0 {
            self.sys.disk_free as f32 / self.sys.disk_total as f32
        } else {
            0.0
        };
        let model_size: u64 = self.models.iter().map(|m| m.total_size()).sum();
        let running = self.running_count();
        let total_services = self.config.services.len();

        let gap = 12.0;
        let width = ((ui.available_width() - gap * 3.0) / 4.0)
            .floor()
            .max(150.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
            stat_card(
                ui,
                width,
                "系统内存",
                &util::human_bytes(used),
                &format!("/ {}", util::human_bytes(self.sys.total_ram)),
                ACCENT,
                Some(ram_ratio),
            );
            stat_card(
                ui,
                width,
                "磁盘可用",
                &util::human_bytes(self.sys.disk_free),
                &format!("/ {}", util::human_bytes(self.sys.disk_total)),
                TEAL,
                Some(disk_ratio),
            );
            stat_card(
                ui,
                width,
                "本地模型",
                &self.models.len().to_string(),
                &format!("个 · {}", util::human_bytes(model_size)),
                ACCENT_TEXT,
                None,
            );
            stat_card(
                ui,
                width,
                "运行中服务",
                &format!("{running}/{total_services}"),
                "个服务",
                if running > 0 { GREEN } else { FAINT },
                Some(if total_services == 0 {
                    0.0
                } else {
                    running as f32 / total_services as f32
                }),
            );
        });
        ui.add_space(6.0);

        let running_services: Vec<(ServiceDef, Status)> = self
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

        if running_services.is_empty() {
            if empty_state(
                ui,
                NavIcon::Status,
                "没有运行中的服务",
                "启动一个服务后，这里会显示吞吐、内存占用与 KV 缓存使用率。",
                Some("前往服务页"),
            ) {
                self.tab = Tab::Services;
            }
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (service, status) in &running_services {
                    card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let healthy = status.healthy();
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&service.name).size(15.0).strong().color(TEXT));
                            pill(
                                ui,
                                if healthy { "健康" } else { "加载中" },
                                if healthy { GREEN } else { AMBER },
                            );
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(format!("http://{}:{}", service.host, service.port))
                                    .monospace()
                                    .size(11.5)
                                    .color(FAINT),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if button(ui, "查看日志", Kind::Quiet).clicked() {
                                    self.tab = Tab::Logs;
                                    self.log_for = Some(service.id.clone());
                                    self.log_lines.clear();
                                    self.reload_log();
                                }
                            });
                        });
                        ui.add_space(12.0);
                        let metrics: Vec<(&str, String)> = vec![
                            ("进程", status.pid.to_string()),
                            ("运行时长", util::duration_label(status.uptime)),
                            (
                                "内存",
                                status
                                    .rss
                                    .map(util::human_bytes)
                                    .unwrap_or_else(|| "—".into()),
                            ),
                            (
                                "上下文",
                                status
                                    .n_ctx()
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "—".into()),
                            ),
                            (
                                "并行槽",
                                status
                                    .slots()
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "—".into()),
                            ),
                            ("已生成", format!("{:.0}", status.tokens())),
                            ("已处理输入", format!("{:.0}", status.prompt_tokens())),
                        ];
                        egui::Frame::new()
                            .fill(SUNKEN)
                            .corner_radius(10)
                            .inner_margin(Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                metric_strip(ui, &metrics);
                            });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("KV 缓存").size(11.0).color(FAINT));
                            let kv = status.kv().map(|v| v as f32);
                            let bar_w = (ui.available_width() - 104.0).max(80.0);
                            ui.add_space(8.0);
                            progress(
                                ui,
                                kv.unwrap_or(0.0),
                                bar_w,
                                kv.map(accent_for).unwrap_or(alpha(Color32::WHITE, 16)),
                            );
                            ui.add_space(8.0);
                            match kv {
                                Some(kv) => ui.label(
                                    RichText::new(format!("{:.0}%", kv * 100.0))
                                        .size(11.0)
                                        .color(TEXT_DIM),
                                ),
                                None => ui.label(RichText::new("暂无数据").size(11.0).color(FAINT)),
                            };
                        });
                        ui.add_space(8.0);
                        if let Some(health) = &status.health {
                            let text = pretty(health);
                            egui::CollapsingHeader::new(
                                RichText::new("健康检查原始响应").size(11.0).color(MUTED),
                            )
                            .id_salt(("health", service.id.as_str()))
                            .show(ui, |ui| {
                                egui::Frame::new()
                                    .fill(SUNKEN)
                                    .corner_radius(8)
                                    .inner_margin(Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.add(
                                            egui::TextEdit::multiline(&mut text.as_str())
                                                .code_editor()
                                                .desired_rows(6)
                                                .desired_width(f32::INFINITY)
                                                .interactive(false),
                                        );
                                    });
                            });
                        }
                        if let Some(err) = &status.error {
                            ui.label(
                                RichText::new(format!("健康检查失败：{err}"))
                                    .size(11.0)
                                    .color(AMBER),
                            );
                        }
                    });
                    ui.add_space(10.0);
                }
            });
    }

    fn logs_tab(&mut self, ui: &mut egui::Ui) {
        if self.config.services.is_empty() {
            if empty_state(
                ui,
                NavIcon::Logs,
                "没有可跟随的服务",
                "创建并启动一个服务后，llama-server 的输出会出现在这里。",
                Some("前往服务页"),
            ) {
                self.tab = Tab::Services;
            }
            return;
        }
        self.ensure_log();
        let current = self.log_for.clone().unwrap_or_default();
        let services = self.config.services.clone();

        ui.horizontal(|ui| {
            let selected = services
                .iter()
                .find(|s| s.id == current)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "选择服务".into());
            egui::ComboBox::from_id_salt("log_service")
                .selected_text(RichText::new(selected).size(12.5).color(TEXT))
                .width(190.0)
                .show_ui(ui, |ui| {
                    for service in &services {
                        if ui
                            .selectable_label(service.id == current, &service.name)
                            .clicked()
                        {
                            self.log_for = Some(service.id.clone());
                            self.log_lines.clear();
                        }
                    }
                });
            if button(ui, "刷新", Kind::Ghost).clicked() {
                self.reload_log();
            }
            if button(ui, "打开日志文件", Kind::Quiet).clicked() {
                if let Some(run) = self.state.services.get(&current) {
                    let _ = std::process::Command::new("open").arg(&run.log).spawn();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                dot_label(
                    ui,
                    if self
                        .status
                        .get(&current)
                        .map(|s| s.running)
                        .unwrap_or(false)
                    {
                        GREEN
                    } else {
                        FAINT
                    },
                    "自动跟随 · 最近 400 行",
                );
            });
        });
        ui.add_space(12.0);

        let size = ui.available_size();
        sunken().show(ui, |ui| {
            ui.set_min_size(size - Vec2::new(26.0, 26.0));
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.log_lines.is_empty() {
                        ui.label(
                            RichText::new("暂无日志。启动服务后这里会自动刷新。")
                                .size(11.5)
                                .color(FAINT),
                        );
                    }
                    for (index, line) in self.log_lines.iter().enumerate() {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            ui.label(
                                RichText::new(format!("{:>4}", index + 1))
                                    .monospace()
                                    .size(10.5)
                                    .color(FAINT),
                            );
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .size(11.5)
                                    .color(log_tone(line)),
                            );
                        });
                    }
                });
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width().min(920.0);
                let pad = ((ui.available_width() - width) * 0.5).max(0.0);
                ui.horizontal_top(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                ui.set_width(width);

                // ---------------------------------------------------- 模型目录
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_header(
                        ui,
                        "模型目录",
                        "递归扫描目录下的 .gguf 文件，自动识别量化、参数与上下文长度。",
                    );
                    ui.add_space(12.0);
                    if self.config.model_dirs.is_empty() {
                        ui.label(
                            RichText::new("还没有添加目录。")
                                .size(11.5)
                                .color(FAINT),
                        );
                    }
                    let dirs = self.config.model_dirs.clone();
                    for (index, dir) in dirs.iter().enumerate() {
                        egui::Frame::new()
                            .fill(SUNKEN)
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                                    folder_icon(ui.painter(), rect, MUTED);
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(util::truncate(
                                            &dir.display().to_string(),
                                            58,
                                        ))
                                        .monospace()
                                        .size(11.5)
                                        .color(TEXT_DIM),
                                    );
                                    ui.with_layout(
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            if icon_button(ui, "✕", "移除目录", Kind::Danger)
                                                .clicked()
                                            {
                                                self.config.model_dirs.remove(index);
                                                actions.push(Action::SaveConfig);
                                                actions.push(Action::Rescan);
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(6.0);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if button(ui, "添加目录", Kind::Ghost).clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                if !self.config.model_dirs.iter().any(|d| d == &dir) {
                                    self.config.model_dirs.push(dir);
                                    actions.push(Action::SaveConfig);
                                    actions.push(Action::Rescan);
                                }
                            }
                        }
                        if button(ui, "重新扫描", Kind::Quiet).clicked() {
                            actions.push(Action::Rescan);
                        }
                    });
                });
                ui.add_space(12.0);

                // ------------------------------------------------- llama-server
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_header(
                        ui,
                        "llama-server",
                        "服务启动时调用的可执行文件；带动态库的发行包会自动设置 DYLD_LIBRARY_PATH。",
                    );
                    ui.add_space(12.0);
                    let found = self.server_path();
                    ui.horizontal(|ui| {
                        pill(
                            ui,
                            if found.is_some() { "已就绪" } else { "未找到" },
                            if found.is_some() { GREEN } else { AMBER },
                        );
                        if let Some(path) = &found {
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(util::truncate(&path.display().to_string(), 62))
                                    .monospace()
                                    .size(11.0)
                                    .color(FAINT),
                            );
                        }
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if button(ui, "选择可执行文件", Kind::Ghost).clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                self.config.llama_server = Some(path);
                                actions.push(Action::SaveConfig);
                            }
                        }
                        if button(ui, "自动查找", Kind::Quiet).clicked() {
                            match crate::config::find_llama_server() {
                                Some(path) => {
                                    self.config.llama_server = Some(path);
                                    actions.push(Action::SaveConfig);
                                }
                                None => self.toast("没有在常见位置找到 llama-server", true),
                            }
                        }
                    });
                });
                ui.add_space(12.0);

                // ------------------------------------------------- 模型下载
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_header(
                        ui,
                        "模型下载",
                        "「发现模型」从这里检索 GGUF 并下载到第一个模型目录；国内网络建议用镜像。",
                    );
                    ui.add_space(12.0);
                    let current = self.config.hub_endpoint.clone();
                    let options = [
                        ("自动（镜像优先）", None),
                        ("hf-mirror", Some(hub::MIRROR)),
                        ("huggingface.co", Some(hub::OFFICIAL)),
                    ];
                    ui.horizontal(|ui| {
                        for (label, value) in options {
                            let selected = current.as_deref() == value;
                            let hit = if selected {
                                button(ui, label, Kind::Primary)
                            } else {
                                button(ui, label, Kind::Ghost)
                            };
                            if hit.clicked() && !selected {
                                self.config.hub_endpoint = value.map(|v| v.to_string());
                                actions.push(Action::SaveConfig);
                            }
                        }
                    });
                    ui.add_space(8.0);
                    let effective = self
                        .hub_bases()
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "未配置".into());
                    hint(ui, &format!("当前下载源：{effective}"));
                });
                ui.add_space(12.0);

                // ------------------------------------------------- 刷新频率
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_header(
                        ui,
                        "刷新频率",
                        "运行状态页轮询健康检查与指标的间隔，数值越大越省电。",
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        slider_u64(ui, &mut self.config.refresh_secs, 1, 30, 300.0);
                        ui.add_space(10.0);
                        chip(ui, &format!("{} 秒", self.config.refresh_secs), None);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if button(ui, "保存配置", Kind::Primary).clicked() {
                                actions.push(Action::SaveConfig);
                            }
                        });
                    });
                    ui.add_space(2.0);
                    hint(ui, "修改后点击保存；未保存前只在本次运行中生效。");
                });
                ui.add_space(12.0);

                // ------------------------------------------------- 文件位置
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_header(ui, "文件位置", "配置、状态与日志都保存在应用数据目录。");
                    ui.add_space(12.0);
                    path_row(ui, "配置文件", &Config::path(), "打开");
                    path_row(ui, "运行状态", &StateStore::path(), "打开");
                    path_row(ui, "日志目录", &runtime::logs_dir(), "打开");
                });
                ui.add_space(4.0);
                });
                });
            });
    }

    // ------------------------------------------------------------ dialogs

    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(mut editor) = self.editor.take() else {
            return;
        };
        let mut submit = false;
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let title = if editor.original_id.is_some() {
            "编辑服务"
        } else {
            "新建服务"
        };
        let window = egui::Window::new(title)
            .id(Id::new("service_editor"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .min_width(540.0)
            .max_width(540.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(14)
                    .inner_margin(Margin::same(22))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 18],
                        blur: 44,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(496.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(16.0).strong().color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, "✕", "关闭", Kind::Quiet).clicked() {
                            cancel = true;
                        }
                    });
                });
                ui.add_space(4.0);
                hint(ui, "保存后写入配置文件，可随时回服务页启动或停止。");
                divider(ui);
                ui.add_space(2.0);

                form_row(ui, "名称", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.name)
                            .hint_text("例如 Qwen3-8B-Q4")
                            .desired_width(360.0)
                            .margin(Margin::symmetric(10, 6)),
                    );
                });
                form_row(ui, "模型文件", |ui| {
                    ui.horizontal(|ui| {
                        let label = if editor.model.as_os_str().is_empty() {
                            "未选择模型".to_string()
                        } else {
                            util::truncate(&editor.model.display().to_string(), 40)
                        };
                        path_box(ui, &label, 300.0);
                        if button(ui, "选择", Kind::Ghost).clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("GGUF", &["gguf"])
                                .pick_file()
                            {
                                editor.model = path;
                            }
                        }
                    });
                });
                form_row(ui, "视觉投影", |ui| {
                    ui.horizontal(|ui| match &editor.mmproj {
                        Some(path) => {
                            path_box(ui, &util::truncate(&path.display().to_string(), 40), 300.0);
                            if button(ui, "清除", Kind::Quiet).clicked() {
                                editor.mmproj = None;
                            }
                        }
                        None => {
                            path_box(ui, "未启用（纯文本模型无需设置）", 300.0);
                            if button(ui, "选择", Kind::Ghost).clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("GGUF", &["gguf"])
                                    .pick_file()
                                {
                                    editor.mmproj = Some(path);
                                }
                            }
                        }
                    });
                });

                ui.add_space(4.0);
                divider(ui);
                ui.label(RichText::new("运行参数").size(11.0).strong().color(MUTED));
                ui.add_space(10.0);
                ui.horizontal_top(|ui| {
                    numeric_field(ui, "端口", |ui| {
                        ui.add_sized(
                            [110.0, 28.0],
                            egui::DragValue::new(&mut editor.port).range(1024..=65535),
                        );
                    });
                    ui.add_space(22.0);
                    numeric_field(ui, "上下文长度", |ui| {
                        ui.add_sized(
                            [130.0, 28.0],
                            egui::DragValue::new(&mut editor.ctx)
                                .range(512..=131072)
                                .speed(256),
                        );
                    });
                    if editor.mmproj.is_some() {
                        ui.add_space(22.0);
                        numeric_field(ui, "图像最小 token", |ui| {
                            ui.add_sized(
                                [110.0, 28.0],
                                egui::DragValue::new(&mut editor.image_min_tokens).range(1..=8192),
                            );
                        });
                    }
                });
                ui.add_space(12.0);
                ui.horizontal_top(|ui| {
                    numeric_field(ui, "GPU 层 (-ngl)", |ui| {
                        ui.add_sized(
                            [110.0, 28.0],
                            egui::DragValue::new(&mut editor.ngl).range(0..=999),
                        );
                    });
                    ui.add_space(22.0);
                    numeric_field(ui, "并行槽 (-np)", |ui| {
                        ui.add_sized(
                            [130.0, 28.0],
                            egui::DragValue::new(&mut editor.parallel).range(1..=8),
                        );
                    });
                });
                ui.add_space(12.0);
                form_row(ui, "额外参数", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.extra)
                            .hint_text("--chat-template ... 以空格分隔")
                            .desired_width(360.0)
                            .margin(Margin::symmetric(10, 6)),
                    );
                });

                if let Some(server) = self.server_path() {
                    let service = editor.to_service();
                    let command = service.command_line(&server).join(" ");
                    egui::CollapsingHeader::new(
                        RichText::new("查看启动命令").size(11.0).color(MUTED),
                    )
                    .id_salt("editor_command")
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .fill(SUNKEN)
                            .corner_radius(8)
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    RichText::new(command)
                                        .monospace()
                                        .size(10.5)
                                        .color(TEXT_DIM),
                                );
                            });
                    });
                }

                divider(ui);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "保存", Kind::Primary).clicked() {
                            submit = true;
                        }
                        ui.add_space(4.0);
                        if button(ui, "取消", Kind::Ghost).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        let _ = window;

        if submit {
            if !editor.model.is_file() {
                self.toast("请选择有效的模型文件", true);
                self.editor = Some(editor);
                return;
            }
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
        } else if !cancel {
            self.editor = Some(editor);
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut confirm_now = false;

        egui::Window::new(&confirm.title)
            .id(Id::new("confirm_dialog"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .min_width(440.0)
            .max_width(440.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(14)
                    .inner_margin(Margin::same(22))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 18],
                        blur: 44,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(396.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(10), alpha(AMBER, 26));
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "!",
                        FontId::proportional(18.0),
                        AMBER,
                    );
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(&confirm.title)
                                .size(15.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(RichText::new("此操作不可撤销").size(10.5).color(FAINT));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, "✕", "关闭", Kind::Quiet).clicked() {
                            cancel = true;
                        }
                    });
                });
                ui.add_space(12.0);
                ui.label(RichText::new(&confirm.body).size(12.0).color(TEXT_DIM));
                if !confirm.detail.is_empty() {
                    ui.add_space(10.0);
                    egui::Frame::new()
                        .fill(SUNKEN)
                        .corner_radius(8)
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .max_height(132.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for line in &confirm.detail {
                                        ui.label(
                                            RichText::new(util::truncate(line, 74))
                                                .monospace()
                                                .size(10.5)
                                                .color(FAINT),
                                        );
                                    }
                                });
                        });
                }
                divider(ui);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "删除", Kind::DangerSolid).clicked() {
                            confirm_now = true;
                        }
                        ui.add_space(4.0);
                        if button(ui, "取消", Kind::Ghost).clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        if confirm_now {
            match confirm.service {
                Some(id) => actions.push(Action::RemoveService(id)),
                None => actions.push(Action::DeleteModel {
                    files: confirm.files,
                    size: confirm.size,
                }),
            }
        } else if !cancel {
            self.confirm = Some(confirm);
        }
    }

    fn hub_picker_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(picker) = self.hub.picker.take() else {
            return;
        };
        let model = picker.model.clone();
        let files = self.hub.files.get(&model).cloned().unwrap_or_default();
        let dir = self.config.model_dirs.first().cloned();
        let recommended = hub::recommend(&files);
        let dir_space = dir.as_ref().and_then(|d| hub::free_space(d));
        let ram = runtime::sys_info().total_ram;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut install: Option<HubFile> = None;

        egui::Window::new("模型文件")
            .id(Id::new("hub_files"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .min_width(580.0)
            .max_width(580.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER))
                    .corner_radius(14)
                    .inner_margin(Margin::same(22))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 18],
                        blur: 44,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(536.0);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(10), alpha(ACCENT, 26));
                    paint_nav_icon(ui.painter(), rect.center(), NavIcon::Discover, ACCENT_TEXT);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(util::truncate(&model, 46))
                                .size(14.5)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new("选择要下载到本地的量化文件")
                                .size(10.5)
                                .color(FAINT),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, "✕", "关闭", Kind::Quiet).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(12.0);

                if picker.loading {
                    egui::Frame::new()
                        .fill(SUNKEN)
                        .corner_radius(10)
                        .inner_margin(Margin::symmetric(14, 18))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("正在读取仓库文件列表…")
                                        .size(12.0)
                                        .color(TEXT_DIM),
                                );
                            });
                        });
                } else if let Some(err) = &picker.error {
                    egui::Frame::new()
                        .fill(alpha(RED, 14))
                        .stroke(Stroke::new(1.0_f32, alpha(RED, 56)))
                        .corner_radius(10)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.label(RichText::new(err).size(11.5).color(TEXT_DIM));
                        });
                } else {
                    egui::Frame::new()
                        .fill(SUNKEN)
                        .corner_radius(10)
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .max_height(320.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for (idx, file) in files.iter().enumerate() {
                                        let installed = self.local_file(&file.name).is_some();
                                        let mut install_now = false;
                                        ui.horizontal_top(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            let right_w = 86.0;
                                            let info_w =
                                                (ui.available_width() - right_w).max(240.0);
                                            region(
                                                ui,
                                                Vec2::new(info_w, 38.0),
                                                Layout::top_down(Align::Min),
                                                |ui| {
                                                    ui.spacing_mut().item_spacing.y = 4.0;
                                                    let mut row = egui::text::LayoutJob::default();
                                                    row.wrap.max_width = info_w - 12.0;
                                                    row.wrap.max_rows = 1;
                                                    row.append(
                                                        &util::truncate(&file.name, 44),
                                                        0.0,
                                                        egui::TextFormat {
                                                            font_id: FontId::proportional(12.0),
                                                            color: if installed {
                                                                MUTED
                                                            } else {
                                                                TEXT
                                                            },
                                                            ..Default::default()
                                                        },
                                                    );
                                                    if Some(idx) == recommended && !installed {
                                                        row.append(
                                                            "  推荐",
                                                            0.0,
                                                            egui::TextFormat {
                                                                font_id: FontId::proportional(10.5),
                                                                color: ACCENT_TEXT,
                                                                ..Default::default()
                                                            },
                                                        );
                                                    }
                                                    if file.is_mmproj() {
                                                        row.append(
                                                            "  视觉投影",
                                                            0.0,
                                                            egui::TextFormat {
                                                                font_id: FontId::proportional(10.5),
                                                                color: TEAL,
                                                                ..Default::default()
                                                            },
                                                        );
                                                    }
                                                    if dir_space
                                                        .map(|free| {
                                                            file.size + 64 * 1024 * 1024 > free
                                                        })
                                                        .unwrap_or(false)
                                                    {
                                                        row.append(
                                                            "  空间不足",
                                                            0.0,
                                                            egui::TextFormat {
                                                                font_id: FontId::proportional(10.5),
                                                                color: RED,
                                                                ..Default::default()
                                                            },
                                                        );
                                                    }
                                                    if ram > 0 && file.size > ram - ram / 10 {
                                                        row.append(
                                                            "  超出内存",
                                                            0.0,
                                                            egui::TextFormat {
                                                                font_id: FontId::proportional(10.5),
                                                                color: AMBER,
                                                                ..Default::default()
                                                            },
                                                        );
                                                    }
                                                    ui.label(row);
                                                    let mut meta =
                                                        hub::meta_line(file.size, &file.name);
                                                    if installed {
                                                        meta.push_str(" · 已在本地");
                                                    }
                                                    ui.label(
                                                        RichText::new(meta).size(10.0).color(FAINT),
                                                    );
                                                },
                                            );
                                            region(
                                                ui,
                                                Vec2::new(right_w, 38.0),
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if installed {
                                                        badge(ui, "已安装", GREEN);
                                                    } else if button(ui, "下载", Kind::Ghost)
                                                        .clicked()
                                                    {
                                                        install_now = true;
                                                    }
                                                },
                                            );
                                        });
                                        if install_now {
                                            install = Some(file.clone());
                                        }
                                        ui.add_space(2.0);
                                    }
                                });
                        });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let target = dir
                        .as_ref()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "未设置模型目录".into());
                    ui.add(
                        egui::Label::new(
                            RichText::new(match dir_space {
                                Some(free) => {
                                    format!("保存到 {target}（可用 {}）", util::human_bytes(free))
                                }
                                None => format!("保存到 {target}"),
                            })
                            .size(10.5)
                            .color(FAINT),
                        )
                        .truncate(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "关闭", Kind::Ghost).clicked() {
                            close = true;
                        }
                    });
                });
            });

        if let Some(file) = install {
            actions.push(Action::HubInstall { model, file });
            self.hub.picker = None;
        } else if !close {
            self.hub.picker = Some(picker);
        }
    }

    fn toast_bar(&mut self, ctx: &egui::Context) {
        let Some((message, error, at)) = self.toast.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_secs(5) {
            self.toast = None;
            return;
        }
        let tone = if error { RED } else { GREEN };
        let mut dismiss = false;
        egui::Area::new(Id::new("toast"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -28.0))
            .order(Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0_f32, alpha(tone, 90)))
                    .corner_radius(11)
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            toast_icon(ui.painter(), rect, !error);
                            ui.add_space(2.0);
                            ui.label(RichText::new(message).size(12.5).color(TEXT).strong());
                            ui.add_space(6.0);
                            if icon_button(ui, "✕", "关闭", Kind::Quiet).clicked() {
                                dismiss = true;
                            }
                        });
                    });
            });
        if dismiss {
            self.toast = None;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
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
            id: self
                .original_id
                .clone()
                .unwrap_or_else(|| crate::config::slug(&self.name)),
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

        // 系统可能恢复出跑到屏幕外的窗口位置，首次渲染时把窗口拉回可视区域。
        if !self.placed {
            self.placed = true;
            if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
                let monitor = ctx
                    .input(|i| i.viewport().monitor_size)
                    .unwrap_or(Vec2::new(1440.0, 900.0));
                let off_screen = rect.max.y <= 0.0
                    || rect.max.x <= 0.0
                    || rect.min.x >= monitor.x
                    || rect.min.y >= monitor.y;
                if off_screen || rect.min.y < 0.0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        120.0, 80.0,
                    )));
                }
            }
        }

        if self.last_poll.elapsed() >= Duration::from_secs(self.config.refresh_secs.max(1)) {
            self.poll();
        }
        self.hub_poll();
        if self.tab == Tab::Discover && !self.hub.searched && !self.hub.loading {
            actions.push(Action::HubSearch);
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

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(24, 20)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Discover => self.discover_tab(ui, &mut actions),
                Tab::Models => self.models_tab(ui, &mut actions),
                Tab::Services => self.services_tab(ui, &mut actions),
                Tab::Status => self.status_tab(ui),
                Tab::Logs => self.logs_tab(ui),
                Tab::Settings => self.settings_tab(ui, &mut actions),
            });

        if self.editor.is_some() || self.confirm.is_some() || self.hub.picker.is_some() {
            theme::scrim(ctx);
        }
        self.editor_window(ctx);
        self.confirm_window(ctx, &mut actions);
        self.hub_picker_window(ctx, &mut actions);

        for action in actions {
            self.apply(action);
        }

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

// ---------------------------------------------------------------- helpers

fn count_label(value: u64) -> String {
    if value >= 100_000_000 {
        format!("{:.1} 亿", value as f64 / 100_000_000.0)
    } else if value >= 10_000 {
        format!("{:.1} 万", value as f64 / 10_000.0)
    } else {
        value.to_string()
    }
}

fn hub_download_card(ui: &mut egui::Ui, download: &hub::Download, actions: &mut Vec<Action>) {
    let done = download.done;
    let ok = download.error.is_none();
    let tone = if done && !ok {
        RED
    } else if done {
        GREEN
    } else {
        ACCENT
    };
    card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let right_w = 170.0;
            let info_w = (ui.available_width() - right_w).max(220.0);
            region(
                ui,
                Vec2::new(46.0, 40.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    if ui.is_rect_visible(rect) {
                        let painter = ui.painter();
                        painter.rect_filled(rect, CornerRadius::same(9), alpha(tone, 24));
                        paint_nav_icon(painter, rect.center(), NavIcon::Discover, tone);
                    }
                    ui.add_space(8.0);
                },
            );
            region(
                ui,
                Vec2::new(info_w, 40.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.label(
                        RichText::new(util::truncate(&download.file, 54))
                            .size(12.5)
                            .strong()
                            .color(TEXT),
                    );
                    ui.add_space(5.0);
                    let (text, color) = match &download.error {
                        Some(err) => (err.clone(), if done { AMBER } else { RED }),
                        None if done => (
                            format!("已保存 · {}", util::human_bytes(download.received)),
                            GREEN,
                        ),
                        None => {
                            let mut text = format!(
                                "{} / {}",
                                util::human_bytes(download.received),
                                util::human_bytes(download.total)
                            );
                            let speed = hub::speed_label(download.speed);
                            if !speed.is_empty() {
                                text.push_str(&format!(" · {speed}"));
                            }
                            if let Some(eta) = download.eta() {
                                text.push_str(&format!(" · {}", hub::eta_label(eta)));
                            }
                            (text, FAINT)
                        }
                    };
                    ui.label(RichText::new(text).size(10.5).color(color));
                },
            );
            region(
                ui,
                Vec2::new(right_w, 40.0),
                Layout::right_to_left(Align::Center),
                |ui| {
                    if done {
                        if button(ui, "移除", Kind::Quiet).clicked() {
                            actions.push(Action::HubForget(download.key()));
                        }
                        if ok {
                            ui.add_space(4.0);
                            if button(ui, "访达", Kind::Quiet).clicked() {
                                actions.push(Action::Reveal(download.dest.clone()));
                            }
                        }
                    } else if button(ui, "取消", Kind::Ghost).clicked() {
                        actions.push(Action::HubCancel(download.key()));
                    }
                },
            );
        });
        if !done {
            ui.add_space(10.0);
            let width = ui.available_width().max(80.0);
            progress(ui, download.ratio(), width, ACCENT);
            ui.add_space(5.0);
            ui.label(
                RichText::new(format!(
                    "{:.0}%  ·  {}",
                    download.ratio() * 100.0,
                    download.dir.display()
                ))
                .size(10.0)
                .color(FAINT),
            );
        }
    });
}

fn nav_item(ui: &mut egui::Ui, label: &str, icon: NavIcon, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = response.hovered();

    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), ACCENT_TINT);
        let bar = Rect::from_min_size(
            Pos2::new(rect.left(), rect.center().y - 9.0),
            Vec2::new(3.0, 18.0),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), ACCENT);
    } else if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(9), CARD);
    }

    let color = if selected {
        ACCENT_TEXT
    } else if hovered {
        TEXT
    } else {
        MUTED
    };
    paint_nav_icon(
        ui.painter(),
        Pos2::new(rect.left() + 22.0, rect.center().y),
        icon,
        color,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 42.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        color,
    );
    response
}

fn form_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(92.0, 30.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().text(
                Pos2::new(rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.0),
                MUTED,
            );
        }
        add(ui);
    });
    ui.add_space(9.0);
}

fn numeric_field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(10.5).color(FAINT));
        ui.add_space(4.0);
        add(ui);
    });
}

fn path_box(ui: &mut egui::Ui, text: &str, width: f32) {
    egui::Frame::new()
        .fill(SUNKEN)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.label(RichText::new(text).monospace().size(11.5).color(TEXT_DIM));
        });
}

fn metric_strip(ui: &mut egui::Ui, items: &[(&str, String)]) {
    let n = items.len().max(1) as f32;
    let gap = 18.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let col = ((rect.width() - gap * (n - 1.0)) / n).max(40.0);
    let painter = ui.painter();
    for (i, (label, value)) in items.iter().enumerate() {
        let x = rect.left() + i as f32 * (col + gap);
        let l = painter.layout_no_wrap((*label).to_owned(), FontId::proportional(10.0), FAINT);
        painter.galley(Pos2::new(x, rect.top()), l, FAINT);
        let v = painter.layout_no_wrap(value.to_owned(), FontId::monospace(13.5), TEXT);
        painter.galley(Pos2::new(x, rect.top() + 19.0), v, TEXT);
    }
}

fn path_row(ui: &mut egui::Ui, label: &str, path: &std::path::Path, action: &str) {
    ui.horizontal(|ui| {
        ui.add_space(2.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(78.0, 24.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().text(
                Pos2::new(rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(11.5),
                MUTED,
            );
        }
        let text = util::truncate(&path.display().to_string(), 60);
        if theme::button(ui, action, theme::Kind::Quiet).clicked() {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
        }
        ui.label(RichText::new(text).monospace().size(10.5).color(FAINT));
    });
    ui.add_space(4.0);
}

fn folder_icon(painter: &egui::Painter, rect: Rect, color: Color32) {
    let tab = Rect::from_min_size(rect.min + Vec2::new(1.0, 2.0), Vec2::new(7.0, 3.0));
    painter.rect_filled(tab, CornerRadius::same(1), alpha(color, 160));
    let body = Rect::from_min_size(rect.min + Vec2::new(0.5, 5.0), Vec2::new(15.0, 9.0));
    painter.rect_stroke(
        body,
        CornerRadius::same(2),
        Stroke::new(1.3_f32, color),
        StrokeKind::Inside,
    );
}

fn model_tooltip(model: &ModelEntry) -> String {
    let mut lines = vec![model.path.display().to_string()];
    match &model.meta {
        Some(meta) => {
            lines.push(format!(
                "GGUF v{} · {} 张量 · 上下文 {} · {} 层 · 嵌入 {}",
                meta.version,
                meta.tensor_count,
                meta.context_length,
                meta.block_count,
                meta.embedding_length
            ));
        }
        None => lines.push(
            model
                .error
                .clone()
                .unwrap_or_else(|| "无法读取元数据".into()),
        ),
    }
    lines.push(format!(
        "{} · {} 个文件",
        util::human_bytes(model.total_size()),
        model.files.len()
    ));
    if let Some(mmproj) = &model.mmproj {
        lines.push(format!("视觉投影: {}", mmproj.display()));
    }
    lines.join("\n")
}

fn quant_tone(quant: &str) -> Color32 {
    if quant.contains("Q4") || quant.contains("Q5") || quant.contains("Q6") {
        GREEN
    } else if quant.contains("Q8") || quant.contains("F16") || quant.contains("BF16") {
        AMBER
    } else {
        MUTED
    }
}

fn accent_for(ratio: f32) -> Color32 {
    if ratio > 0.85 {
        RED
    } else if ratio > 0.6 {
        AMBER
    } else {
        ACCENT
    }
}

fn log_tone(line: &str) -> Color32 {
    let upper = line.to_ascii_uppercase();
    if upper.contains("ERROR") || upper.contains("FAILED") || upper.contains("PANIC") {
        return RED;
    }
    if upper.contains("WARN") || upper.contains("WARNING") {
        return AMBER;
    }
    // llama.cpp logs the level as a single letter token: "0.12.3 I srv ...".
    for token in line.split_whitespace() {
        match token {
            "E" => return RED,
            "W" => return AMBER,
            _ => {}
        }
    }
    TEXT_DIM
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
