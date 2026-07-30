use std::env;
use std::path::PathBuf;
use std::process;

mod config;
mod logging;
mod signals;
mod supervisor;
mod systemd;
mod windows;

fn config_path() -> PathBuf {
    env::var("AIGOSD_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.yaml"))
}

fn main() {
    let config_path = config_path();

    let cfg = match config::load_config(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[FATAL] Config load failed: {}", e);
            process::exit(1);
        }
    };

    if let Err(error) = logging::init(&cfg.options.logging, cfg.options.log_file.as_deref()) {
        eprintln!("[FATAL] Logger initialization failed: {}", error);
        process::exit(1);
    }
    logging::info(&format!("Using config at: {}", config_path.display()));
    if let Err(error) = logging::ensure_healthy() {
        eprintln!("[FATAL] Initial daemon log write failed: {}", error);
        process::exit(1);
    }

    let mut supervisor = supervisor::Supervisor::new(cfg);
    let mut exit_code = match supervisor.run() {
        Ok(()) => 0,
        Err(e) => {
            logging::error(&format!("Supervisor terminated: {}", e));
            2
        }
    };

    drop(supervisor);
    if logging::write_failure_detected() {
        eprintln!("[FATAL] One or more daemon log writes failed");
        if exit_code == 0 {
            exit_code = 3;
        }
    }
    if exit_code != 0 {
        process::exit(exit_code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn restore_env(previous: Option<OsString>) {
        if let Some(value) = previous {
            env::set_var("AIGOSD_CONFIG", value);
        } else {
            env::remove_var("AIGOSD_CONFIG");
        }
    }

    #[test]
    fn config_path_defaults_to_local_config_only() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let previous = env::var_os("AIGOSD_CONFIG");
        env::remove_var("AIGOSD_CONFIG");

        assert_eq!(config_path(), PathBuf::from("config.yaml"));

        restore_env(previous);
    }

    #[test]
    fn config_path_honors_explicit_environment_override() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let previous = env::var_os("AIGOSD_CONFIG");
        env::set_var("AIGOSD_CONFIG", "custom.yaml");

        assert_eq!(config_path(), PathBuf::from("custom.yaml"));

        restore_env(previous);
    }
}
