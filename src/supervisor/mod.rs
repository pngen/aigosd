use aigos::{is_core_layer, is_extension_layer, is_valid_layer, CANONICAL_CORE_LAYERS};
use indexmap::IndexMap;
use std::error::Error;
use std::fmt;
use std::io::{self, BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{Config, MeshConfig};
use crate::logging;
use crate::signals;

const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const RESTART_DELAY: Duration = Duration::from_secs(3);
#[cfg(unix)]
const PROCESS_GROUP_TERM_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;
#[cfg(unix)]
const ESRCH: i32 = 3;

#[cfg(test)]
const TEST_EXTENSION_LAYERS: &[&str] = &["iam", "sck"];

fn canonical_core_layers() -> &'static [&'static str] {
    CANONICAL_CORE_LAYERS
}

#[cfg(test)]
fn is_valid_runtime_layer(name: &str) -> bool {
    is_valid_layer(name) || TEST_EXTENSION_LAYERS.contains(&name)
}

#[cfg(not(test))]
fn is_valid_runtime_layer(name: &str) -> bool {
    is_valid_layer(name)
}

#[cfg(test)]
fn is_core_runtime_layer(name: &str) -> bool {
    is_core_layer(name)
}

#[cfg(not(test))]
fn is_core_runtime_layer(name: &str) -> bool {
    is_core_layer(name)
}

#[cfg(test)]
fn is_extension_runtime_layer(name: &str) -> bool {
    is_extension_layer(name) || TEST_EXTENSION_LAYERS.contains(&name)
}

#[cfg(not(test))]
fn is_extension_runtime_layer(name: &str) -> bool {
    is_extension_layer(name)
}

#[cfg(unix)]
extern "C" {
    fn setsid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingCoreLayer {
    pub layer: String,
    pub attempted_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingCoreLayersError {
    missing: Vec<MissingCoreLayer>,
}

impl MissingCoreLayersError {
    pub fn missing(&self) -> &[MissingCoreLayer] {
        &self.missing
    }
}

impl fmt::Display for MissingCoreLayersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Missing required layer binaries:")?;
        for missing in &self.missing {
            let attempted = missing
                .attempted_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(f, "- {} (attempted: {})", missing.layer, attempted)?;
        }
        Ok(())
    }
}

impl Error for MissingCoreLayersError {}

#[cfg(unix)]
fn configure_child_process_group(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            if setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_child_process_group(_cmd: &mut Command) {}

#[cfg(unix)]
fn kill_process_group(process_group_id: i32, signal: i32) -> io::Result<()> {
    let rc = unsafe { kill(-process_group_id, signal) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn log_process_group_signal_error(signal_name: &str, handle: &ProcessHandle, error: io::Error) {
    if error.raw_os_error() == Some(ESRCH) {
        return;
    }

    logging::warn(&format!(
        "Failed to send {} to process group {} for {}: {}",
        signal_name, handle.process_group_id, handle.name, error
    ));
}

pub struct Supervisor {
    config: Config,
    processes: IndexMap<String, ProcessHandle>,
    shutdown: Arc<AtomicBool>,
    layer_binaries: IndexMap<String, PathBuf>,
    #[cfg(windows)]
    job: Option<crate::windows::JobObject>,
}

struct ProcessHandle {
    name: String,
    pid: u32,
    layer: String,
    mesh: String,
    child: Child,
    started_at: Instant,
    restart_count: u32,
    output_readers: Vec<thread::JoinHandle<()>>,
    #[cfg(unix)]
    process_group_id: i32,
}

impl ProcessHandle {
    fn join_output_readers(&mut self) {
        for reader in self.output_readers.drain(..) {
            if reader.join().is_err() {
                logging::warn(&format!("Output reader thread panicked for {}", self.name));
            }
        }
    }
}

fn start_output_readers(process_name: &str, child: &mut Child) -> Vec<thread::JoinHandle<()>> {
    let mut readers = Vec::new();

    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_output_reader(
            process_name.to_string(),
            "stdout",
            BufReader::new(stdout),
        ));
    }

    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_output_reader(
            process_name.to_string(),
            "stderr",
            BufReader::new(stderr),
        ));
    }

    readers
}

fn spawn_output_reader<R>(
    process_name: String,
    stream_name: &'static str,
    reader: BufReader<R>,
) -> thread::JoinHandle<()>
where
    R: io::Read + Send + 'static,
{
    thread::spawn(move || {
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    let attributed = format!("[{}:{}] {}", process_name, stream_name, line);
                    if stream_name == "stderr" {
                        logging::error(&attributed);
                    } else {
                        logging::info(&attributed);
                    }
                }
                Err(e) => {
                    logging::warn(&format!(
                        "[{}:{}] output reader failed: {}",
                        process_name, stream_name, e
                    ));
                    break;
                }
            }
        }
    })
}

impl Supervisor {
    pub fn new(config: Config) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        Self {
            config,
            processes: IndexMap::new(),
            shutdown,
            layer_binaries: IndexMap::new(),
            #[cfg(windows)]
            job: None,
        }
    }

    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        logging::info("Supervisor starting");

        signals::reset_shutdown();
        signals::install_shutdown_handler()?;
        let planned_mesh_layers = self.planned_mesh_layers()?;
        self.layer_binaries = Self::resolve_required_layers(
            planned_mesh_layers
                .iter()
                .flat_map(|(_, layers)| layers.iter().map(String::as_str)),
        )?;
        self.ensure_windows_job()?;

        let result = self.run_supervision_loop(planned_mesh_layers);

        logging::info("Supervisor shutting down");
        self.stop_all();
        result
    }

    fn run_supervision_loop(
        &mut self,
        planned_mesh_layers: Vec<(String, Vec<String>)>,
    ) -> Result<(), Box<dyn Error>> {
        for (mesh_name, layers) in planned_mesh_layers {
            self.start_mesh_layers(&mesh_name, &layers)?;
        }

        while !self.shutdown.load(Ordering::SeqCst) && !signals::shutdown_requested() {
            self.poll_processes()?;
            thread::sleep(HEALTH_CHECK_INTERVAL);
        }

        Ok(())
    }

    fn poll_processes(&mut self) -> Result<(), Box<dyn Error>> {
        let restart_policy = self.config.options.restart.clone();
        let mut exited: Vec<(String, Option<(String, String)>)> = Vec::new();

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
                        exited.push((
                            key.clone(),
                            Some((handle.mesh.clone(), handle.layer.clone())),
                        ));
                    } else {
                        logging::info(&format!("{} exited ({}), no restart", key, status));
                        exited.push((key.clone(), None));
                    }
                }
                Ok(None) => {}
                Err(e) => logging::error(&format!("Failed to poll {}: {}", key, e)),
            }
        }

        for (key, restart) in exited {
            if let Some(mut handle) = self.processes.shift_remove(&key) {
                handle.join_output_readers();
            }
            if let Some((mesh, layer)) = restart {
                thread::sleep(RESTART_DELAY);
                self.start_layer(&mesh, &layer)?;
            }
        }

        Ok(())
    }

    pub fn start_mesh(&mut self, mesh_name: &str) -> Result<(), Box<dyn Error>> {
        let layers: Vec<String> = if let Some(mesh_config) = self.config.meshes.get(mesh_name) {
            Self::layers_for_mesh(mesh_config)?
        } else {
            return Ok(());
        };

        let resolved_layers = Self::resolve_required_layers(layers.iter().map(String::as_str))?;
        for (layer, path) in resolved_layers {
            self.layer_binaries.entry(layer).or_insert(path);
        }

        self.start_mesh_layers(mesh_name, &layers)
    }

    fn start_mesh_layers(
        &mut self,
        mesh_name: &str,
        layers: &[String],
    ) -> Result<(), Box<dyn Error>> {
        for layer in layers {
            self.start_layer(mesh_name, layer)?;
        }
        Ok(())
    }

    fn planned_mesh_layers(&self) -> Result<Vec<(String, Vec<String>)>, Box<dyn Error>> {
        let mut planned = Vec::new();
        for (mesh_name, mesh_config) in &self.config.meshes {
            planned.push((mesh_name.clone(), Self::layers_for_mesh(mesh_config)?));
        }
        Ok(planned)
    }

    pub fn layers_for_mesh(mesh_config: &MeshConfig) -> Result<Vec<String>, Box<dyn Error>> {
        let mut layers: Vec<String> = canonical_core_layers()
            .iter()
            .map(|layer| (*layer).to_string())
            .collect();

        let Some(configured_layers) = &mesh_config.layers else {
            return Ok(layers);
        };

        let mut seen_core = IndexMap::new();
        let mut seen_extensions = IndexMap::new();
        let mut extension_layers = Vec::new();

        for layer in configured_layers {
            if !is_valid_runtime_layer(layer.as_str()) {
                return Err(format!("Invalid layer '{}'", layer).into());
            }

            if is_core_runtime_layer(layer.as_str()) {
                if seen_core.insert(layer.as_str(), ()).is_some() {
                    return Err(format!("Duplicate Core layer '{}'", layer).into());
                }
            } else if is_extension_runtime_layer(layer.as_str()) {
                if seen_extensions.insert(layer.as_str(), ()).is_some() {
                    return Err(format!("Duplicate extension layer '{}'", layer).into());
                }
                extension_layers.push(layer.clone());
            }
        }

        if !seen_core.is_empty()
            && (seen_core.len() != canonical_core_layers().len()
                || canonical_core_layers()
                    .iter()
                    .any(|layer| !seen_core.contains_key(*layer)))
        {
            return Err("Partial Core layer list is not allowed".into());
        }

        layers.extend(extension_layers);
        Ok(layers)
    }

    pub fn stop_mesh(&mut self, mesh_name: &str) {
        let prefix = format!("aigosd-{}@", mesh_name);
        let to_stop: Vec<_> = self
            .processes
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();

        for key in to_stop {
            if let Some(handle) = self.processes.shift_remove(&key) {
                self.stop_process(handle);
            }
        }
    }

    fn stop_all(&mut self) {
        let keys: Vec<_> = self.processes.keys().cloned().collect();
        for key in keys {
            if let Some(handle) = self.processes.shift_remove(&key) {
                self.stop_process(handle);
            }
        }
    }

    fn stop_process(&self, mut handle: ProcessHandle) {
        logging::info(&format!(
            "Stopping {} [PID {}, restarts {}]",
            handle.name, handle.pid, handle.restart_count
        ));

        match handle.child.try_wait() {
            Ok(Some(status)) => {
                logging::info(&format!("{} already exited ({})", handle.name, status));
                handle.join_output_readers();
                return;
            }
            Ok(None) => {}
            Err(e) => logging::warn(&format!(
                "Failed to inspect {} before stop: {}",
                handle.name, e
            )),
        }

        self.terminate_process_group_or_child(&mut handle);

        if let Err(e) = handle.child.wait() {
            logging::warn(&format!("Failed to reap {}: {}", handle.name, e));
        }
        handle.join_output_readers();
    }

    #[cfg(unix)]
    fn terminate_process_group_or_child(&self, handle: &mut ProcessHandle) {
        if let Err(e) = kill_process_group(handle.process_group_id, SIGTERM) {
            log_process_group_signal_error("SIGTERM", handle, e);
        }

        let deadline = Instant::now() + PROCESS_GROUP_TERM_TIMEOUT;
        while Instant::now() < deadline {
            match handle.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    logging::warn(&format!(
                        "Failed to inspect {} after SIGTERM: {}",
                        handle.name, e
                    ));
                    break;
                }
            }
        }

        if let Err(e) = kill_process_group(handle.process_group_id, SIGKILL) {
            log_process_group_signal_error("SIGKILL", handle, e);
        }
    }

    #[cfg(not(unix))]
    fn terminate_process_group_or_child(&self, handle: &mut ProcessHandle) {
        if let Err(e) = handle.child.kill() {
            logging::warn(&format!("Failed to kill {}: {}", handle.name, e));
        }
    }

    pub fn restart_mesh(&mut self, mesh_name: &str) -> Result<(), Box<dyn Error>> {
        self.stop_mesh(mesh_name);
        self.start_mesh(mesh_name)
    }

    pub fn start_layer(&mut self, mesh_name: &str, layer_name: &str) -> Result<(), Box<dyn Error>> {
        let process_name = format!("aigosd-{}@{}", mesh_name, layer_name);

        if self.processes.contains_key(&process_name) {
            logging::warn(&format!("{} already running", process_name));
            return Ok(());
        }

        self.ensure_windows_job()?;

        let bin_path = self.layer_binary_path(layer_name)?;
        println!("AIGOSD trying to spawn: {}", bin_path.display());
        let mut cmd = Command::new(&bin_path);
        cmd.arg("--mesh").arg(mesh_name);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        configure_child_process_group(&mut cmd);

        match cmd.spawn() {
            Ok(child) => {
                let child = self.assign_child_to_platform_supervisor(child, &process_name)?;
                let mut child = child;
                let output_readers = start_output_readers(&process_name, &mut child);
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
                        output_readers,
                        #[cfg(unix)]
                        process_group_id: pid as i32,
                    },
                );
                logging::info(&format!("Started {} [PID {}]", process_name, pid));
                Ok(())
            }
            Err(e) => Err(io::Error::new(
                e.kind(),
                format!(
                    "Failed to start {} from {}: {}",
                    process_name,
                    bin_path.display(),
                    e
                ),
            )
            .into()),
        }
    }

    #[cfg(windows)]
    fn assign_child_to_platform_supervisor(
        &self,
        mut child: Child,
        process_name: &str,
    ) -> Result<Child, Box<dyn Error>> {
        if let Some(job) = self.job.as_ref() {
            if let Err(e) = job.assign_child(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to assign {} to Windows job object: {}",
                        process_name, e
                    ),
                )
                .into());
            }
        }

        Ok(child)
    }

    #[cfg(not(windows))]
    fn assign_child_to_platform_supervisor(
        &self,
        child: Child,
        _process_name: &str,
    ) -> Result<Child, Box<dyn Error>> {
        Ok(child)
    }

    fn layer_binary_path(&self, layer_name: &str) -> Result<PathBuf, MissingCoreLayersError> {
        self.layer_binaries
            .get(layer_name)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| Self::resolve_layer_binary(layer_name))
    }

    pub fn resolve_core_layers() -> Result<IndexMap<String, PathBuf>, MissingCoreLayersError> {
        Self::resolve_required_layers(canonical_core_layers().iter().copied())
    }

    fn resolve_required_layers<'a, I>(
        layers: I,
    ) -> Result<IndexMap<String, PathBuf>, MissingCoreLayersError>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut resolved = IndexMap::new();
        let mut missing = Vec::new();

        for layer in layers {
            if resolved.contains_key(layer) {
                continue;
            }

            let attempted_paths = Self::candidate_layer_paths(layer);
            if let Some(path) = attempted_paths.iter().find(|path| path.is_file()) {
                resolved.insert(layer.to_string(), path.clone());
            } else {
                missing.push(MissingCoreLayer {
                    layer: layer.to_string(),
                    attempted_paths,
                });
            }
        }

        if missing.is_empty() {
            Ok(resolved)
        } else {
            Err(MissingCoreLayersError { missing })
        }
    }

    fn resolve_layer_binary(layer_name: &str) -> Result<PathBuf, MissingCoreLayersError> {
        let attempted_paths = Self::candidate_layer_paths(layer_name);
        if let Some(path) = attempted_paths.iter().find(|path| path.is_file()) {
            Ok(path.clone())
        } else {
            Err(MissingCoreLayersError {
                missing: vec![MissingCoreLayer {
                    layer: layer_name.to_string(),
                    attempted_paths,
                }],
            })
        }
    }

    fn candidate_layer_paths(layer_name: &str) -> Vec<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            vec![
                PathBuf::from(".").join(format!("{}.exe", layer_name)),
                PathBuf::from(".")
                    .join(layer_name)
                    .join(format!("{}.exe", layer_name)),
            ]
        }

        #[cfg(not(target_os = "windows"))]
        {
            vec![
                PathBuf::from(".").join(layer_name),
                PathBuf::from(".").join(layer_name).join(layer_name),
            ]
        }
    }

    #[cfg(windows)]
    fn ensure_windows_job(&mut self) -> Result<(), Box<dyn Error>> {
        if self.job.is_none() {
            self.job = Some(crate::windows::JobObject::new()?);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn ensure_windows_job(&mut self) -> Result<(), Box<dyn Error>> {
        Ok(())
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

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_config, Config, MeshConfig, Options};
    use indexmap::IndexMap;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process::{self, Command};
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    struct RuntimeDir {
        path: PathBuf,
        original: PathBuf,
    }

    impl RuntimeDir {
        fn new() -> Self {
            let original = env::current_dir().expect("current dir");
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let path = env::temp_dir().join(format!("aigosd-test-{}-{}", process::id(), unique));
            fs::create_dir_all(&path).expect("create test runtime dir");
            env::set_current_dir(&path).expect("switch to test runtime dir");
            Self { path, original }
        }
    }

    impl Drop for RuntimeDir {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.original);
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn flat_layout_resolves_before_nested_layout() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let _runtime = RuntimeDir::new();

        for layer in canonical_core_layers() {
            create_empty_executable(&Supervisor::candidate_layer_paths(layer)[0]);
            create_empty_executable(&Supervisor::candidate_layer_paths(layer)[1]);
        }

        let resolved = Supervisor::resolve_core_layers().expect("flat layout should resolve");
        for layer in canonical_core_layers() {
            assert_eq!(
                resolved.get(*layer),
                Some(&Supervisor::candidate_layer_paths(layer)[0])
            );
        }
    }

    #[test]
    fn nested_layout_still_resolves() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let _runtime = RuntimeDir::new();

        for layer in canonical_core_layers() {
            create_empty_executable(&Supervisor::candidate_layer_paths(layer)[1]);
        }

        let resolved = Supervisor::resolve_core_layers().expect("nested layout should resolve");
        for layer in canonical_core_layers() {
            assert_eq!(
                resolved.get(*layer),
                Some(&Supervisor::candidate_layer_paths(layer)[1])
            );
        }
    }

    #[test]
    fn config_rejects_partial_core_layer_lists() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let config_path = runtime.path.join("config.yaml");
        fs::write(
            &config_path,
            r#"
meshes:
  mesh:
    layers:
      - dio

options:
  logging: plaintext
  restart: never
"#,
        )
        .expect("write partial config");

        let err = load_config(&config_path).expect_err("partial Core config should be rejected");
        assert!(err
            .to_string()
            .contains("must include all ten canonical Core layers or omit Core layers"));
    }

    #[test]
    fn config_rejects_duplicate_mesh_names_before_indexmap_overwrite() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let config_path = runtime.path.join("config.yaml");
        fs::write(
            &config_path,
            r#"
meshes:
  mesh3: {}
  mesh2: {}
  mesh2: {}

options:
  logging: plaintext
  restart: never
"#,
        )
        .expect("write duplicate mesh config");

        let err = load_config(&config_path).expect_err("duplicate mesh config should be rejected");
        assert!(err
            .to_string()
            .contains("Duplicate mesh name 'mesh2' in config.yaml"));
    }

    #[test]
    fn config_rejects_duplicate_core_layers() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let config_path = runtime.path.join("config.yaml");
        let mut layers = canonical_core_layers()
            .iter()
            .map(|layer| format!("      - {}", layer))
            .collect::<Vec<_>>();
        layers.insert(1, "      - dio".to_string());
        fs::write(
            &config_path,
            format!(
                r#"
meshes:
  mesh:
    layers:
{}

options:
  logging: plaintext
  restart: never
"#,
                layers.join("\n")
            ),
        )
        .expect("write duplicate Core config");

        let err = load_config(&config_path).expect_err("duplicate Core config should be rejected");
        assert!(err
            .to_string()
            .contains("Duplicate Core layer 'dio' in mesh 'mesh'"));
    }

    #[test]
    fn config_rejects_unknown_layers() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let config_path = runtime.path.join("config.yaml");
        fs::write(
            &config_path,
            r#"
meshes:
  mesh:
    layers:
      - made-up-layer

options:
  logging: plaintext
  restart: never
"#,
        )
        .expect("write unknown layer config");

        let err = load_config(&config_path).expect_err("unknown layer config should be rejected");
        assert!(err
            .to_string()
            .contains("Invalid layer 'made-up-layer' in mesh 'mesh'"));
    }

    #[test]
    fn duplicate_extension_layers_are_rejected() {
        let mesh_config = MeshConfig {
            layers: Some(vec!["iam".to_string(), "iam".to_string()]),
        };
        let err = Supervisor::layers_for_mesh(&mesh_config)
            .expect_err("duplicate extension should be rejected");

        assert!(err.to_string().contains("Duplicate extension layer 'iam'"));
    }

    #[test]
    fn config_rejects_duplicate_extension_layers() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let config_path = runtime.path.join("config.yaml");
        fs::write(
            &config_path,
            r#"
meshes:
  mesh:
    layers:
      - iam
      - iam

options:
  logging: plaintext
  restart: never
"#,
        )
        .expect("write duplicate extension config");

        let err =
            load_config(&config_path).expect_err("duplicate extension config should be rejected");
        assert!(err
            .to_string()
            .contains("Duplicate extension layer 'iam' in mesh 'mesh'"));
    }

    #[test]
    fn extension_only_layers_append_after_core_in_config_order() {
        let mesh_config = MeshConfig {
            layers: Some(vec!["iam".to_string(), "sck".to_string()]),
        };
        let layers = Supervisor::layers_for_mesh(&mesh_config).expect("extension layers");
        let expected = expected_layers_with_extensions(&["iam", "sck"]);

        assert_eq!(layers, expected);
    }

    #[test]
    fn mixed_full_core_and_extensions_runs_core_once_then_extensions() {
        let mut configured_layers = canonical_core_layers()
            .iter()
            .map(|layer| (*layer).to_string())
            .collect::<Vec<_>>();
        configured_layers.push("sck".to_string());
        configured_layers.push("iam".to_string());

        let mesh_config = MeshConfig {
            layers: Some(configured_layers),
        };
        let layers = Supervisor::layers_for_mesh(&mesh_config).expect("mixed layers");
        let expected = expected_layers_with_extensions(&["sck", "iam"]);

        assert_eq!(layers, expected);
    }

    #[test]
    fn configured_mesh_spawns_all_canonical_core_layers() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let helper = compile_sleeping_helper(&runtime.path);

        for layer in canonical_core_layers() {
            let path = Supervisor::candidate_layer_paths(layer)[0].clone();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create layer parent");
            }
            fs::copy(&helper, &path).expect("copy helper to layer path");
        }

        let mut supervisor = Supervisor::new(test_config(None));
        supervisor.layer_binaries =
            Supervisor::resolve_core_layers().expect("resolve helper layers");
        supervisor.start_mesh("mesh").expect("start Core mesh");

        for layer in canonical_core_layers() {
            let process_name = format!("aigosd-mesh@{}", layer);
            assert!(
                supervisor.process_exists(&process_name),
                "{} should be running",
                process_name
            );
        }

        drop(supervisor);
    }

    #[test]
    fn explicit_full_core_list_spawns_core_only_once() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let helper = compile_sleeping_helper(&runtime.path);

        for layer in canonical_core_layers() {
            let path = Supervisor::candidate_layer_paths(layer)[0].clone();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create layer parent");
            }
            fs::copy(&helper, &path).expect("copy helper to layer path");
        }

        let mut supervisor = Supervisor::new(test_config(Some(canonical_core_layers())));
        supervisor.layer_binaries =
            Supervisor::resolve_core_layers().expect("resolve helper layers");
        supervisor.start_mesh("mesh").expect("start Core mesh");

        assert_eq!(supervisor.processes.len(), canonical_core_layers().len());
        for layer in canonical_core_layers() {
            let process_name = format!("aigosd-mesh@{}", layer);
            assert!(
                supervisor.process_exists(&process_name),
                "{} should be running once",
                process_name
            );
        }

        drop(supervisor);
    }

    #[test]
    fn configured_extension_missing_binary_fails_before_spawn() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let _runtime = RuntimeDir::new();

        for layer in canonical_core_layers() {
            create_empty_executable(&Supervisor::candidate_layer_paths(layer)[0]);
        }

        let mut supervisor = Supervisor::new(test_config(Some(&["iam"])));
        let err = supervisor
            .run()
            .expect_err("missing configured extension binary should fail");

        assert!(err.to_string().contains("Missing required layer binaries"));
        assert!(err.to_string().contains("- iam (attempted:"));
        assert!(supervisor.processes.is_empty());
    }

    #[test]
    fn planned_mesh_order_follows_config_order() {
        let supervisor = Supervisor::new(test_config_with_meshes(&[
            ("mesh3", None),
            ("mesh2", Some(&["iam"])),
            ("mesh1", Some(canonical_core_layers())),
        ]));
        let planned = supervisor.planned_mesh_layers().expect("planned layers");
        let mesh_names = planned
            .iter()
            .map(|(mesh_name, _)| mesh_name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(mesh_names, vec!["mesh3", "mesh2", "mesh1"]);
        assert_eq!(planned[1].1, expected_layers_with_extensions(&["iam"]));
    }

    #[test]
    fn missing_mandatory_core_layers_fail_before_spawning() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let _runtime = RuntimeDir::new();
        create_empty_executable(&Supervisor::candidate_layer_paths(canonical_core_layers()[0])[0]);

        let mut supervisor = Supervisor::new(test_config(None));
        let err = supervisor.run().expect_err("missing layers should fail");
        let missing = err
            .downcast_ref::<MissingCoreLayersError>()
            .expect("missing Core error");

        assert!(err.to_string().contains("Missing required layer binaries"));
        assert_eq!(missing.missing().len(), canonical_core_layers().len() - 1);
        assert!(supervisor.processes.is_empty());
    }

    #[test]
    fn spawned_children_are_terminated_when_supervisor_drops() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let helper = compile_sleeping_helper(&runtime.path);

        for layer in canonical_core_layers() {
            let path = Supervisor::candidate_layer_paths(layer)[0].clone();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create layer parent");
            }
            fs::copy(&helper, &path).expect("copy helper to layer path");
        }

        let mut supervisor = Supervisor::new(test_config(None));
        supervisor.layer_binaries =
            Supervisor::resolve_core_layers().expect("resolve helper layers");
        supervisor
            .start_layer("mesh", canonical_core_layers()[0])
            .expect("start helper layer");

        let process_name = format!("aigosd-mesh@{}", canonical_core_layers()[0]);
        let pid = supervisor
            .processes
            .get(&process_name)
            .expect("started process")
            .pid;
        assert!(
            process_is_running(pid),
            "child should be running before drop"
        );

        drop(supervisor);
        wait_until_stopped(pid);
        assert!(
            !process_is_running(pid),
            "child should be stopped after drop"
        );
    }

    #[test]
    fn child_output_is_logged_with_process_attribution() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let helper = compile_output_helper(&runtime.path);
        let log_path = runtime.path.join("aigosd-output.log");
        let log_path_string = log_path.display().to_string();
        crate::logging::init("plaintext", Some(&log_path_string));

        let layer = canonical_core_layers()[0];
        let path = Supervisor::candidate_layer_paths(layer)[0].clone();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create layer parent");
        }
        fs::copy(&helper, &path).expect("copy helper to layer path");

        crate::logging::info("supervisor-smoke-before");
        let mut supervisor = Supervisor::new(test_config(None));
        supervisor
            .start_layer("mesh", layer)
            .expect("start output helper layer");
        crate::logging::info("supervisor-smoke-after");

        let process_name = format!("aigosd-mesh@{}", layer);
        let mut handle = supervisor
            .processes
            .shift_remove(&process_name)
            .expect("started process");
        let status = handle.child.wait().expect("wait for output helper");
        handle.join_output_readers();
        assert!(status.success(), "output helper should exit successfully");

        let contents = fs::read_to_string(&log_path).expect("read output log");
        let stdout_line = format!("[{}:stdout] child stdout alpha", process_name);
        let stderr_line = format!("[{}:stderr] child stderr beta", process_name);

        assert!(
            contents.contains(&stdout_line),
            "stdout attribution missing"
        );
        assert!(
            contents.contains(&stderr_line),
            "stderr attribution missing"
        );
        assert!(contents.contains("supervisor-smoke-before"));
        assert!(contents.contains("supervisor-smoke-after"));

        for line in contents.lines() {
            assert!(
                !line.contains("child stdout alpha") || line.contains(&stdout_line),
                "stdout appeared without attribution: {}",
                line
            );
            assert!(
                !line.contains("child stderr beta") || line.contains(&stderr_line),
                "stderr appeared without attribution: {}",
                line
            );
            assert!(
                !(line.contains("supervisor-smoke") && line.contains("child stdout alpha")),
                "child stdout interleaved with supervisor log record: {}",
                line
            );
            assert!(
                !(line.contains("supervisor-smoke") && line.contains("child stderr beta")),
                "child stderr interleaved with supervisor log record: {}",
                line
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_terminates_spawned_process_group() {
        let _lock = CWD_LOCK.lock().expect("cwd lock");
        let runtime = RuntimeDir::new();
        let helper = compile_process_group_helper(&runtime.path);
        let grandchild_pid_file = runtime.path.join("grandchild.pid");

        for layer in canonical_core_layers() {
            let path = Supervisor::candidate_layer_paths(layer)[0].clone();
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create layer parent");
            }
            fs::copy(&helper, &path).expect("copy helper to layer path");
        }

        env::set_var("AIGOSD_TEST_GRANDCHILD_PID", &grandchild_pid_file);
        let mut supervisor = Supervisor::new(test_config(None));
        supervisor.layer_binaries =
            Supervisor::resolve_core_layers().expect("resolve helper layers");
        supervisor
            .start_layer("mesh", canonical_core_layers()[0])
            .expect("start process-group helper layer");
        env::remove_var("AIGOSD_TEST_GRANDCHILD_PID");

        let process_name = format!("aigosd-mesh@{}", canonical_core_layers()[0]);
        let pid = supervisor
            .processes
            .get(&process_name)
            .expect("started process")
            .pid;
        let grandchild_pid = wait_for_pid_file(&grandchild_pid_file);

        assert!(
            process_is_running(pid),
            "child should be running before drop"
        );
        assert!(
            process_is_running(grandchild_pid),
            "grandchild should be running before drop"
        );

        drop(supervisor);
        wait_until_stopped(pid);
        wait_until_stopped(grandchild_pid);
        assert!(
            !process_is_running(pid),
            "child should be stopped after drop"
        );
        assert!(
            !process_is_running(grandchild_pid),
            "grandchild process group member should be stopped after drop"
        );
    }

    fn test_config(layers: Option<&[&str]>) -> Config {
        test_config_with_meshes(&[("mesh", layers)])
    }

    fn test_config_with_meshes(mesh_specs: &[(&str, Option<&[&str]>)]) -> Config {
        let mut meshes = IndexMap::new();
        for (mesh_name, layers) in mesh_specs {
            meshes.insert(
                (*mesh_name).to_string(),
                MeshConfig {
                    layers: layers
                        .map(|layers| layers.iter().map(|layer| (*layer).to_string()).collect()),
                },
            );
        }

        Config {
            meshes,
            options: Options {
                logging: "plaintext".to_string(),
                restart: "never".to_string(),
                log_file: None,
            },
        }
    }

    fn expected_layers_with_extensions(extensions: &[&str]) -> Vec<String> {
        let mut layers = canonical_core_layers()
            .iter()
            .map(|layer| (*layer).to_string())
            .collect::<Vec<_>>();
        layers.extend(extensions.iter().map(|layer| (*layer).to_string()));
        layers
    }

    fn create_empty_executable(path: &PathBuf) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, b"").expect("create executable placeholder");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("set executable bit");
        }
    }

    fn compile_sleeping_helper(runtime: &PathBuf) -> PathBuf {
        let source = runtime.join("sleeping_layer.rs");
        let output = runtime.join(if cfg!(windows) {
            "sleeping_layer.exe"
        } else {
            "sleeping_layer"
        });
        fs::write(
            &source,
            r#"
fn main() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
"#,
        )
        .expect("write helper source");

        let status = Command::new("rustc")
            .arg(&source)
            .arg("-O")
            .arg("-o")
            .arg(&output)
            .status()
            .expect("run rustc for helper");
        assert!(status.success(), "helper binary should compile");
        output
    }

    fn compile_output_helper(runtime: &PathBuf) -> PathBuf {
        let source = runtime.join("output_layer.rs");
        let output = runtime.join(if cfg!(windows) {
            "output_layer.exe"
        } else {
            "output_layer"
        });
        fs::write(
            &source,
            r#"
fn main() {
    println!("child stdout alpha");
    eprintln!("child stderr beta");
}
"#,
        )
        .expect("write output helper source");

        let status = Command::new("rustc")
            .arg(&source)
            .arg("-O")
            .arg("-o")
            .arg(&output)
            .status()
            .expect("run rustc for output helper");
        assert!(status.success(), "output helper binary should compile");
        output
    }

    #[cfg(unix)]
    fn compile_process_group_helper(runtime: &PathBuf) -> PathBuf {
        let source = runtime.join("process_group_layer.rs");
        let output = runtime.join("process_group_layer");
        fs::write(
            &source,
            r#"
use std::env;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    let pid_file = env::var("AIGOSD_TEST_GRANDCHILD_PID").expect("pid file env");
    let child = Command::new("sh")
        .arg("-c")
        .arg("trap '' TERM; while true; do sleep 60; done")
        .spawn()
        .expect("spawn grandchild");
    fs::write(pid_file, child.id().to_string()).expect("write grandchild pid");

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
"#,
        )
        .expect("write process-group helper source");

        let status = Command::new("rustc")
            .arg(&source)
            .arg("-O")
            .arg("-o")
            .arg(&output)
            .status()
            .expect("run rustc for process-group helper");
        assert!(
            status.success(),
            "process-group helper binary should compile"
        );
        output
    }

    #[cfg(unix)]
    fn wait_for_pid_file(path: &PathBuf) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(contents) = fs::read_to_string(path) {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    return pid;
                }
            }

            assert!(
                Instant::now() < deadline,
                "timed out waiting for pid file {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_until_stopped(pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while process_is_running(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    fn process_is_running(pid: u32) -> bool {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }

        unsafe { kill(pid as i32, 0) == 0 }
    }

    #[cfg(windows)]
    fn process_is_running(pid: u32) -> bool {
        use std::ffi::c_void;
        use std::ptr;

        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const SYNCHRONIZE: u32 = 0x0010_0000;
        const WAIT_TIMEOUT: u32 = 0x0000_0102;

        extern "system" {
            fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
            fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }

        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
        if handle == ptr::null_mut() {
            return false;
        }

        let wait_result = unsafe { WaitForSingleObject(handle, 0) };
        unsafe {
            CloseHandle(handle);
        }
        wait_result == WAIT_TIMEOUT
    }
}
