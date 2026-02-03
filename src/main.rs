use std::env;
use std::path::PathBuf;
use std::process;

mod supervisor;
mod logging;
mod config;
mod systemd;
mod windows;

fn main() {
let config_path = env::var("AIGOSD_CONFIG")
    .map(PathBuf::from)
    .unwrap_or_else(|_| {
        let local = PathBuf::from("config.yaml");
        if local.exists() {
            return local;
        }

        if cfg!(windows) {
            PathBuf::from(r"C:\ProgramData\aigos\config.yaml")
        } else {
            PathBuf::from("/etc/aigos/config.yaml")
        }
    });

    logging::info(&format!("Using config at: {}", config_path.display()));

    let cfg = match config::load_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[FATAL] Config load failed: {}", e);
            process::exit(1);
        }
    };

    logging::init(&cfg.options.logging, cfg.options.log_file.as_deref());

    let mut supervisor = supervisor::Supervisor::new(cfg);
    if let Err(e) = supervisor.run() {
        logging::error(&format!("Supervisor terminated: {}", e));
        process::exit(2);
    }
}