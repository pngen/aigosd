use std::env;
use std::path::PathBuf;
use std::process;

mod config;
mod logging;
mod signals;
mod supervisor;
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
    let exit_code = match supervisor.run() {
        Ok(()) => 0,
        Err(e) => {
            logging::error(&format!("Supervisor terminated: {}", e));
            2
        }
    };

    drop(supervisor);
    if exit_code != 0 {
        process::exit(exit_code);
    }
}
