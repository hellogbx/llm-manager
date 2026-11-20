//! `llmmgr` — local LLM manager.
//!
//! No arguments opens the desktop UI. Subcommands give the same features a CLI.

mod config;
mod gguf;
mod library;
mod runtime;
mod ui;
mod util;

use std::path::PathBuf;
use std::process::ExitCode;

use config::{Config, ServiceDef};
use library::ModelEntry;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(|s| s.as_str()).unwrap_or("gui");
    let rest = &args[args.len().min(1)..];

    match command {
        "gui" | "" => match ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("无法启动界面: {err}");
                ExitCode::FAILURE
            }
        },
        "help" | "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        "list" | "models" => cmd_list(rest),
        "services" => cmd_services(rest),
        "start" => cmd_start(rest, true),
        "stop" => cmd_start(rest, false),
        "restart" => cmd_restart(rest),
        "status" => cmd_status(rest),
        "new" => cmd_new(rest),
        "rm-service" => cmd_rm_service(rest),
        "delete" => cmd_delete(rest),
        "add-dir" => cmd_add_dir(rest),
        "set-server" => cmd_set_server(rest),
        "paths" => {
            println!("配置: {}", Config::path().display());
            println!("状态: {}", runtime::StateStore::path().display());
            println!("日志: {}", runtime::logs_dir().display());
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("未知命令: {other}\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "llmmgr — 本地大模型管理器

用法:
  llmmgr                          打开图形界面
  llmmgr list [过滤词]            列出模型
  llmmgr services                 列出服务及运行状态
  llmmgr start <服务> | stop <服务>
  llmmgr restart <服务>
  llmmgr new --name 名称 --model 路径 [--port 8080] [--ctx 4096] [--ngl 99] [--mmproj 路径]
  llmmgr rm-service <服务>
  llmmgr status [--json]          运行中服务的状态
  llmmgr delete <模型路径|文件名> [--yes]
  llmmgr add-dir <目录>
  llmmgr set-server <llama-server 路径|auto>
  llmmgr paths                    显示配置/状态/日志位置

环境变量:
  LLMMGR_HOME     覆盖配置目录
  LLAMA_SERVER    llama-server 可执行文件路径"
    );
}

fn require_config() -> Config {
    Config::load()
}

fn load_models(config: &Config) -> Vec<ModelEntry> {
    library::scan(&config.model_dirs).0
}

fn cmd_list(args: &[String]) -> ExitCode {
    let config = require_config();
    let filter = args.join(" ").to_ascii_lowercase();
    let models = load_models(&config);
    if models.is_empty() {
        println!("未找到模型。用 `llmmgr add-dir <目录>` 添加模型目录。");
        println!("当前目录: {:?}", config.model_dirs);
        return ExitCode::SUCCESS;
    }
    println!(
        "{:<44} {:>9} {:>7} {:>8} {:<12} {}",
        "模型", "大小", "参数", "量化", "架构", "路径"
    );
    let mut total = 0u64;
    for model in &models {
        if !filter.is_empty() && !model.file_name.to_ascii_lowercase().contains(&filter) {
            continue;
        }
        total += model.total_size();
        println!(
            "{:<44} {:>9} {:>7} {:>8} {:<12} {}",
            util::truncate(&model.display_name(), 43),
            util::human_bytes(model.total_size()),
            model.params(),
            model.quant(),
            model.arch(),
            model.path.display()
        );
    }
    println!("\n共 {} 个模型, 合计 {}", models.len(), util::human_bytes(total));
    ExitCode::SUCCESS
}

fn cmd_services(_args: &[String]) -> ExitCode {
    let config = require_config();
    let mut state = runtime::StateStore::load();
    state.prune();
    if config.services.is_empty() {
        println!("还没有服务。用 `llmmgr new --name 名称 --model 路径` 创建，或在界面里新建。");
        return ExitCode::SUCCESS;
    }
    for service in &config.services {
        let running = state
            .services
            .get(&service.id)
            .filter(|run| runtime::is_alive(run.pid));
        let status = match running {
            Some(run) => {
                let health = runtime::health(&service.host, service.port)
                    .map(|v| v.get("status").and_then(|s| s.as_str()).unwrap_or("?").to_string())
                    .unwrap_or_else(|_| "加载中".into());
                format!("运行中 pid {} {} ({}s)", run.pid, health, util::now_unix() - run.started_at)
            }
            None => "已停止".to_string(),
        };
        println!(
            "{:<20} {:<16} {:>5}  {:<28} {}",
            service.id,
            util::truncate(&service.name, 15),
            service.port,
            util::truncate(&service.model.display().to_string(), 27),
            status
        );
    }
    ExitCode::SUCCESS
}

fn cmd_start(args: &[String], start: bool) -> ExitCode {
    let Some(id) = args.first() else {
        eprintln!("用法: llmmgr {} <服务id>", if start { "start" } else { "stop" });
        return ExitCode::FAILURE;
    };
    let config = require_config();
    let Some(service) = config.service(id).cloned() else {
        eprintln!("找不到服务: {id}");
        return ExitCode::FAILURE;
    };
    let mut state = runtime::StateStore::load();
    state.prune();
    if start {
        match runtime::start(&config, &service, &mut state) {
            Ok(pid) => {
                println!("已启动 {} (pid {pid}), 端口 {}", service.name, service.port);
                match wait_ready(&service, 60) {
                    Ok(()) => println!("服务就绪: http://{}:{}", service.host, service.port),
                    Err(err) => {
                        eprintln!("启动后未能就绪: {err}");
                        return ExitCode::FAILURE;
                    }
                }
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("启动失败: {err}");
                ExitCode::FAILURE
            }
        }
    } else {
        match runtime::stop(&service.id, &mut state) {
            Ok(()) => {
                println!("已停止 {}", service.name);
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("停止失败: {err}");
                ExitCode::FAILURE
            }
        }
    }
}

fn cmd_restart(args: &[String]) -> ExitCode {
    let Some(id) = args.first() else {
        eprintln!("用法: llmmgr restart <服务id>");
        return ExitCode::FAILURE;
    };
    let mut state = runtime::StateStore::load();
    let config = require_config();
    let Some(service) = config.service(id).cloned() else {
        eprintln!("找不到服务: {id}");
        return ExitCode::FAILURE;
    };
    let _ = runtime::stop(&service.id, &mut state);
    std::thread::sleep(std::time::Duration::from_millis(400));
    cmd_start(&[id.clone()], true)
}

fn cmd_status(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let config = require_config();
    let mut state = runtime::StateStore::load();
    state.prune();

    let mut rows = Vec::new();
    for service in &config.services {
        let Some(run) = state.services.get(&service.id) else {
            continue;
        };
        if !runtime::is_alive(run.pid) {
            continue;
        }
        let health = runtime::health(&service.host, service.port).ok();
        let rss = runtime::rss_bytes(run.pid);
        rows.push(serde_json::json!({
            "id": service.id,
            "name": service.name,
            "port": service.port,
            "pid": run.pid,
            "uptime_secs": util::now_unix().saturating_sub(run.started_at),
            "rss_bytes": rss,
            "health": health,
            "model": service.model.display().to_string(),
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    if rows.is_empty() {
        println!("没有正在运行的服务。");
        return ExitCode::SUCCESS;
    }
    for row in rows {
        println!(
            "{} (pid {}, 端口 {}) 运行 {} 内存 {}",
            row["name"].as_str().unwrap_or("?"),
            row["pid"],
            row["port"],
            util::duration_label(row["uptime_secs"].as_u64().unwrap_or(0)),
            row["rss_bytes"]
                .as_u64()
                .map(util::human_bytes)
                .unwrap_or_else(|| "?".into())
        );
    }
    ExitCode::SUCCESS
}

fn wait_ready(service: &ServiceDef, timeout_secs: u64) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    while std::time::Instant::now() < deadline {
        if let Ok(value) = runtime::health(&service.host, service.port) {
            if value.get("status").and_then(|s| s.as_str()) == Some("ok") {
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(format!("{timeout_secs} 秒内未就绪，查看日志"))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|idx| args.get(idx + 1).cloned())
}

fn cmd_new(args: &[String]) -> ExitCode {
    let (Some(name), Some(model)) = (flag(args, "--name"), flag(args, "--model")) else {
        eprintln!("用法: llmmgr new --name 名称 --model 路径 [--port 8080] [--ctx 4096] [--ngl 99] [--mmproj 路径]");
        return ExitCode::FAILURE;
    };
    let mut config = require_config();
    let mut service = ServiceDef::new(&name, &PathBuf::from(&model));
    if let Some(port) = flag(args, "--port").and_then(|p| p.parse().ok()) {
        service.port = port;
    } else {
        service.port = config.next_free_port();
    }
    if let Some(ctx) = flag(args, "--ctx").and_then(|p| p.parse().ok()) {
        service.ctx = ctx;
    }
    if let Some(ngl) = flag(args, "--ngl").and_then(|p| p.parse().ok()) {
        service.ngl = ngl;
    }
    if let Some(np) = flag(args, "--parallel").and_then(|p| p.parse().ok()) {
        service.parallel = np;
    }
    if let Some(mmproj) = flag(args, "--mmproj") {
        service.mmproj = Some(PathBuf::from(mmproj));
        service.image_min_tokens = Some(1024);
    }
    config.add_service(service.clone());
    match config.save() {
        Ok(()) => {
            println!("已创建服务 {} (端口 {})", service.id, service.port);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("保存失败: {err}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_rm_service(args: &[String]) -> ExitCode {
    let Some(id) = args.first() else {
        eprintln!("用法: llmmgr rm-service <服务id>");
        return ExitCode::FAILURE;
    };
    let mut config = require_config();
    let mut state = runtime::StateStore::load();
    let Some(service) = config.service(id).cloned() else {
        eprintln!("找不到服务: {id}");
        return ExitCode::FAILURE;
    };
    let _ = runtime::stop(&service.id, &mut state);
    config.services.retain(|s| s.id != service.id);
    match config.save() {
        Ok(()) => {
            println!("已删除服务 {}", service.id);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("保存失败: {err}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_delete(args: &[String]) -> ExitCode {
    let Some(target) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("用法: llmmgr delete <模型路径|文件名> [--yes]");
        return ExitCode::FAILURE;
    };
    let yes = args.iter().any(|a| a == "--yes");
    let config = require_config();
    let models = load_models(&config);
    let target_lc = target.to_lowercase();
    let mut hits: Vec<&library::ModelEntry> = models
        .iter()
        .filter(|m| {
            m.path.display().to_string() == *target
                || m.file_name == *target
                || m.display_name() == *target
        })
        .collect();
    if hits.is_empty() {
        hits = models
            .iter()
            .filter(|m| {
                m.display_name().to_lowercase().contains(&target_lc)
                    || m.file_name.to_lowercase().contains(&target_lc)
                    || m.path.display().to_string().to_lowercase().contains(&target_lc)
            })
            .collect();
    }
    let model = match hits.len() {
        0 => {
            eprintln!("找不到模型: {target}");
            return ExitCode::FAILURE;
        }
        1 => hits[0],
        _ => {
            eprintln!("匹配到 {} 个模型，请写得更具体:", hits.len());
            for item in &hits {
                eprintln!("  {}", item.display_name());
            }
            return ExitCode::FAILURE;
        }
    };
    let files = model.all_files();
    let size = model.total_size();
    println!("将删除 {} 个文件, 释放 {}:", files.len(), util::human_bytes(size));
    for file in &files {
        println!("  {}", file.display());
    }
    if !yes {
        eprintln!("未确认删除，请加 --yes");
        return ExitCode::FAILURE;
    }
    match library::delete_files(&files) {
        Ok(freed) => {
            println!("已删除, 释放 {}", util::human_bytes(freed));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("删除失败: {err}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_add_dir(args: &[String]) -> ExitCode {
    let Some(dir) = args.first() else {
        eprintln!("用法: llmmgr add-dir <目录>");
        return ExitCode::FAILURE;
    };
    let path = PathBuf::from(dir);
    if !path.is_dir() {
        eprintln!("不是目录: {}", path.display());
        return ExitCode::FAILURE;
    }
    let mut config = require_config();
    if !config.model_dirs.contains(&path) {
        config.model_dirs.push(path.clone());
    }
    match config.save() {
        Ok(()) => {
            println!("已添加模型目录 {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("保存失败: {err}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_set_server(args: &[String]) -> ExitCode {
    let Some(raw) = args.first() else {
        eprintln!("用法: llmmgr set-server <llama-server 路径|auto>");
        return ExitCode::FAILURE;
    };
    let mut config = require_config();
    if raw == "auto" {
        match config::find_llama_server() {
            Some(found) => {
                println!("找到 {}", found.display());
                config.llama_server = Some(found);
            }
            None => {
                eprintln!("没有自动找到 llama-server，请显式给出路径");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let path = PathBuf::from(raw);
        if !path.is_file() {
            eprintln!("不是文件: {}", path.display());
            return ExitCode::FAILURE;
        }
        config.llama_server = Some(path);
    }
    match config.save() {
        Ok(()) => {
            if let Some(path) = &config.llama_server {
                println!("llama-server: {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("保存失败: {err}");
            ExitCode::FAILURE
        }
    }
}
