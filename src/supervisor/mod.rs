use indexmap::IndexMap;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use crate::config::Config;
use crate::logging;

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const RESTART_DELAY: Duration = Duration::from_secs(3);

pub struct Supervisor {
    config: Config,
    processes: IndexMap<String, ProcessHandle>,
    shutdown: Arc<AtomicBool>,
}

struct ProcessHandle {
    name: String,
    pid: u32,
    layer: String,
    mesh: String,
    child: Child,
    started_at: Instant,
    restart_count: u32,
}

impl Supervisor {
    pub fn new(config: Config) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        Self {
            config,
            processes: IndexMap::new(),
            shutdown,
        }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        logging::info("Supervisor starting");
        
    let mesh_names: Vec<String> = self.config
        .meshes
        .keys()
        .cloned()
        .collect();

    for mesh_name in mesh_names {
        self.start_mesh(&mesh_name);
    }
        
        while !self.shutdown.load(Ordering::SeqCst) {
            self.poll_processes();
            thread::sleep(HEALTH_CHECK_INTERVAL);
        }
        
        logging::info("Supervisor shutting down");
        self.stop_all();
        Ok(())
    }
    
    fn poll_processes(&mut self) {
        let restart_policy = self.config.options.restart.clone();
        let mut to_restart: Vec<(String, String)> = Vec::new();
        
        for (key, handle) in self.processes.iter_mut() {
            match handle.child.try_wait() {
                Ok(Some(status)) => {
                    let should_restart = match restart_policy.as_str() {
                        "always" => true,
                        "on-failure" => !status.success(),
                        _ => false,
                    };
                    if should_restart {
                        logging::warn(&format!("{} exited ({}), scheduling restart", key, status));
                        to_restart.push((handle.mesh.clone(), handle.layer.clone()));
                    } else {
                        logging::info(&format!("{} exited ({}), no restart", key, status));
                    }
                }
                Ok(None) => {} // still running
                Err(e) => logging::error(&format!("Failed to poll {}: {}", key, e)),
            }
        }
        
        for (mesh, layer) in to_restart {
            let key = format!("aigosd-{}@{}", mesh, layer);
            self.processes.shift_remove(&key);
            thread::sleep(RESTART_DELAY);
            self.start_layer(&mesh, &layer);
        }
    }

    pub fn start_mesh(&mut self, mesh_name: &str) {
        if let Some(mesh_config) = self.config.meshes.get(mesh_name) {
            let layers: Vec<_> = mesh_config.layers.clone();
            for layer in layers {
                self.start_layer(mesh_name, &layer);
            }
        }
    }

    pub fn stop_mesh(&mut self, mesh_name: &str) {
        let to_stop: Vec<_> = self.processes
            .keys()
            .filter(|key| key.starts_with(&format!("{}@", mesh_name)))
            .cloned()
            .collect();

        for key in to_stop {
            if let Some(mut handle) = self.processes.shift_remove(&key) {
                logging::info(&format!("Stopping {}", handle.name));
                let _ = handle.child.kill();
                let _ = handle.child.wait();
            }
        }
    }
    
    fn stop_all(&mut self) {
        let keys: Vec<_> = self.processes.keys().cloned().collect();
        for key in keys {
            if let Some(mut handle) = self.processes.shift_remove(&key) {
                logging::info(&format!("Stopping {}", handle.name));
                let _ = handle.child.kill();
                let _ = handle.child.wait();
            }
        }
    }

    pub fn restart_mesh(&mut self, mesh_name: &str) {
        self.stop_mesh(mesh_name);
        self.start_mesh(mesh_name);
    }

    pub fn start_layer(&mut self, mesh_name: &str, layer_name: &str) {
        let process_name = format!("aigosd-{}@{}", mesh_name, layer_name);

        if self.processes.contains_key(&process_name) {
            logging::warn(&format!("{} already running", process_name));
            return;
        }

        let bin_path = Self::resolve_layer_binary(layer_name);
        println!("AIGOSD trying to spawn: {}", bin_path.display());
        let mut cmd = Command::new(&bin_path);
        cmd.arg("--mesh").arg(mesh_name);

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                let name_clone = process_name.clone();
                self.processes.insert(
                    process_name.clone(),
                    ProcessHandle {
                        name: name_clone,
                        pid,
                        layer: layer_name.to_string(),
                        mesh: mesh_name.to_string(),
                        child,
                        started_at: Instant::now(),
                        restart_count: 0,
                    }
                );
                logging::info(&format!("Started {} [PID {}]", process_name, pid));
            },
            Err(e) => logging::error(&format!("Failed to start {}: {}", process_name, e)),
        }
    }

    fn resolve_layer_binary(layer_name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let mut p = PathBuf::new();
        p.push(layer_name);
        p.push(format!("{}.exe", layer_name));
        p
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut p = PathBuf::new();
        p.push(layer_name);
        p.push(layer_name);
        p
    }
}

    pub fn process_exists(&self, name: &str) -> bool {
        self.processes.contains_key(name)
    }

    pub fn health_check(&self, name: &str) -> bool {
        if let Some(handle) = self.processes.get(name) {
            handle.started_at.elapsed() > Duration::from_secs(1)
        } else {
            false
        }
    }
}