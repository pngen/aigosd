use indexmap::IndexMap;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path};

use aigos::{is_core_layer, is_extension_layer, is_valid_layer, CANONICAL_CORE_LAYERS};

const SUPPORTED_CONFIG_VERSION: &str = "1.0.0";

#[cfg(test)]
const TEST_EXTENSION_LAYERS: &[&str] = &["iam", "sck"];

fn canonical_core_layers() -> &'static [&'static str] {
    CANONICAL_CORE_LAYERS
}

#[cfg(test)]
fn is_valid_config_layer(name: &str) -> bool {
    is_valid_layer(name) || TEST_EXTENSION_LAYERS.contains(&name)
}

#[cfg(not(test))]
fn is_valid_config_layer(name: &str) -> bool {
    is_valid_layer(name)
}

#[cfg(test)]
fn is_core_config_layer(name: &str) -> bool {
    is_core_layer(name)
}

#[cfg(not(test))]
fn is_core_config_layer(name: &str) -> bool {
    is_core_layer(name)
}

#[cfg(test)]
fn is_extension_config_layer(name: &str) -> bool {
    is_extension_layer(name) || TEST_EXTENSION_LAYERS.contains(&name)
}

#[cfg(not(test))]
fn is_extension_config_layer(name: &str) -> bool {
    is_extension_layer(name)
}

#[derive(Serialize, Debug)]
pub struct Config {
    pub meshes: IndexMap<String, MeshConfig>,
    pub options: Options,
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ConfigVisitor)
    }
}

struct ConfigVisitor;

impl<'de> Visitor<'de> for ConfigVisitor {
    type Value = Config;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AIGOSD config mapping")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen_keys = HashSet::new();
        let mut version = None;
        let mut meshes = None;
        let mut options = None;

        while let Some(key) = map.next_key::<String>()? {
            if !seen_keys.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "Duplicate top-level key '{}' in config.yaml",
                    key
                )));
            }

            match key.as_str() {
                "version" => {
                    version = Some(map.next_value::<String>()?);
                }
                "meshes" => {
                    meshes = Some(map.next_value_seed(MeshesSeed)?);
                }
                "options" => {
                    options = Some(map.next_value()?);
                }
                _ => {
                    return Err(de::Error::unknown_field(
                        &key,
                        &["version", "meshes", "options"],
                    ));
                }
            }
        }

        if let Some(version) = version {
            if version != SUPPORTED_CONFIG_VERSION {
                return Err(de::Error::custom(format!(
                    "Unsupported config version '{version}'; expected '{SUPPORTED_CONFIG_VERSION}'"
                )));
            }
        }

        Ok(Config {
            meshes: meshes.ok_or_else(|| de::Error::missing_field("meshes"))?,
            options: options.ok_or_else(|| de::Error::missing_field("options"))?,
        })
    }
}

struct MeshesSeed;

impl<'de> DeserializeSeed<'de> for MeshesSeed {
    type Value = IndexMap<String, MeshConfig>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(MeshesVisitor)
    }
}

struct MeshesVisitor;

impl<'de> Visitor<'de> for MeshesVisitor {
    type Value = IndexMap<String, MeshConfig>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mesh mapping")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut meshes = IndexMap::new();

        while let Some(mesh_name) = map.next_key::<String>()? {
            if meshes.contains_key(&mesh_name) {
                return Err(de::Error::custom(format!(
                    "Duplicate mesh name '{}' in config.yaml",
                    mesh_name
                )));
            }

            let mesh_config = map.next_value()?;
            meshes.insert(mesh_name, mesh_config);
        }

        Ok(meshes)
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct MeshConfig {
    #[serde(default)]
    pub layers: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub logging: String,
    pub restart: String,
    #[serde(default)]
    pub log_file: Option<String>,
}

pub fn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)?;
    let config: Config = serde_yml::from_str(&contents)?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    if config.meshes.is_empty() {
        return Err("Config must define at least one mesh".into());
    }

    for (mesh_name, mesh_cfg) in &config.meshes {
        validate_mesh_name(mesh_name)?;
        if let Some(layers) = &mesh_cfg.layers {
            validate_layer_list(mesh_name, layers)?;
        }
    }

    if !["structured", "plaintext"].contains(&config.options.logging.as_str()) {
        return Err(format!("Invalid logging mode: {}", config.options.logging).into());
    }
    if !["on-failure", "never", "always"].contains(&config.options.restart.as_str()) {
        return Err(format!("Invalid restart policy: {}", config.options.restart).into());
    }
    if let Some(log_file) = &config.options.log_file {
        validate_log_file_path(log_file)?;
    }
    Ok(())
}

fn validate_mesh_name(mesh_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = mesh_name.bytes();
    let first = bytes.next();
    if mesh_name.len() > 64
        || !matches!(first, Some(byte) if byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    {
        return Err(format!(
            "Invalid mesh name '{mesh_name}': use 1-64 ASCII characters, start with a letter or digit, and use only letters, digits, '_', '.', or '-'"
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_log_file_path(log_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_root = fs::canonicalize(env::current_dir()?)?;
    validate_log_file_path_under(&runtime_root, Path::new(log_file))
}

fn validate_log_file_path_under(
    runtime_root: &Path,
    log_file: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if log_file.as_os_str().is_empty() || log_file.is_absolute() {
        return Err(
            "Log file must be a non-empty relative path inside the runtime directory".into(),
        );
    }

    let mut normal_components = 0usize;
    for component in log_file.components() {
        match component {
            Component::Normal(_) => normal_components += 1,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(
                    "Log file path must not contain parent, root, or platform-prefix components"
                        .into(),
                );
            }
        }
    }
    if normal_components == 0 {
        return Err("Log file path must name a file inside the runtime directory".into());
    }

    let canonical_root = fs::canonicalize(runtime_root)?;
    let candidate = canonical_root.join(log_file);
    let mut cursor = canonical_root.clone();
    for component in log_file.components() {
        if let Component::Normal(part) = component {
            cursor.push(part);
            match fs::symlink_metadata(&cursor) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "Log file path must not traverse a symbolic link: {}",
                        cursor.display()
                    )
                    .into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    let parent = candidate
        .parent()
        .ok_or("Log file path must have a local parent directory")?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("Log file parent resolves outside the runtime directory".into());
    }

    match fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err("Log file must not be a symbolic link".into());
            }
            if !metadata.is_file() {
                return Err("Configured log path must be a regular file".into());
            }
            let canonical_candidate = fs::canonicalize(&candidate)?;
            if !canonical_candidate.starts_with(&canonical_root) {
                return Err("Log file resolves outside the runtime directory".into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    Ok(())
}

fn validate_layer_list(
    mesh_name: &str,
    layers: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if layers.is_empty() {
        return Err(format!(
            "Mesh '{}' must omit layers or list Core and/or extension layers",
            mesh_name
        )
        .into());
    }

    let mut seen_core = HashSet::new();
    let mut seen_extensions = HashSet::new();

    for layer in layers {
        if !is_valid_config_layer(layer.as_str()) {
            return Err(format!("Invalid layer '{}' in mesh '{}'", layer, mesh_name).into());
        }

        if is_core_config_layer(layer.as_str()) {
            if !seen_core.insert(layer.as_str()) {
                return Err(
                    format!("Duplicate Core layer '{}' in mesh '{}'", layer, mesh_name).into(),
                );
            }
        } else if is_extension_config_layer(layer.as_str())
            && !seen_extensions.insert(layer.as_str())
        {
            return Err(format!(
                "Duplicate extension layer '{}' in mesh '{}'",
                layer, mesh_name
            )
            .into());
        }
    }

    if !seen_core.is_empty()
        && (seen_core.len() != canonical_core_layers().len()
            || canonical_core_layers()
                .iter()
                .any(|layer| !seen_core.contains(*layer)))
    {
        return Err(format!(
            "Core mesh '{}' must include all ten canonical Core layers or omit Core layers",
            mesh_name
        )
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn parse_and_validate(yaml: &str) -> Result<Config, Box<dyn std::error::Error>> {
        let config: Config = serde_yml::from_str(yaml)?;
        validate(&config)?;
        Ok(config)
    }

    fn valid_yaml(prefix: &str) -> String {
        format!(
            r#"{prefix}
meshes:
  mesh1: {{}}
options:
  logging: plaintext
  restart: never
"#
        )
    }

    fn config_with_mesh(mesh_name: &str) -> Config {
        let mut meshes = IndexMap::new();
        meshes.insert(mesh_name.to_string(), MeshConfig { layers: None });
        Config {
            meshes,
            options: Options {
                logging: "plaintext".to_string(),
                restart: "never".to_string(),
                log_file: None,
            },
        }
    }

    fn temporary_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "aigosd-config-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temporary config root");
        root
    }

    #[test]
    fn config_accepts_absent_or_supported_version() {
        parse_and_validate(&valid_yaml("")).expect("version may be absent");
        parse_and_validate(&valid_yaml("version: \"1.0.0\""))
            .expect("supported version should parse");
    }

    #[test]
    fn config_rejects_unsupported_version_and_unknown_top_level_fields() {
        let unsupported = parse_and_validate(&valid_yaml("version: \"2.0.0\""))
            .expect_err("unsupported version must fail");
        assert!(unsupported
            .to_string()
            .contains("Unsupported config version"));

        let unknown = parse_and_validate(&valid_yaml("unexpected: true"))
            .expect_err("unknown top-level field must fail");
        assert!(unknown.to_string().contains("unknown field"));
    }

    #[test]
    fn config_rejects_unknown_mesh_and_option_fields() {
        let unknown_mesh = parse_and_validate(
            r#"
meshes:
  mesh1:
    layer: dio
options:
  logging: plaintext
  restart: never
"#,
        )
        .expect_err("unknown mesh field must fail");
        assert!(unknown_mesh.to_string().contains("unknown field"));

        let unknown_option = parse_and_validate(
            r#"
meshes:
  mesh1: {}
options:
  logging: plaintext
  restart: never
  restart_delay: 1
"#,
        )
        .expect_err("unknown option field must fail");
        assert!(unknown_option.to_string().contains("unknown field"));
    }

    #[test]
    fn mesh_names_are_canonical_and_unambiguous() {
        validate(&config_with_mesh("mesh-1.prod_core")).expect("valid mesh name");

        for invalid in [
            "",
            "-mesh",
            "mesh@other",
            "mesh/other",
            "mésh",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            let error = validate(&config_with_mesh(invalid)).expect_err("invalid mesh name");
            assert!(error.to_string().contains("mesh name"));
        }
    }

    #[test]
    fn log_file_is_confined_to_a_regular_local_path() {
        let root = temporary_root("log-path");
        let logs = root.join("logs");
        fs::create_dir(&logs).expect("create logs directory");

        validate_log_file_path_under(&root, Path::new("logs/aigosd.log"))
            .expect("new local log file should be accepted");
        assert!(validate_log_file_path_under(&root, &root.join("absolute.log")).is_err());
        assert!(validate_log_file_path_under(&root, Path::new("../escape.log")).is_err());
        assert!(validate_log_file_path_under(&root, Path::new("missing/aigosd.log")).is_err());

        fs::remove_dir_all(&root).expect("remove temporary config root");
    }

    #[cfg(unix)]
    #[test]
    fn log_file_rejects_symbolic_link_components() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("log-symlink");
        let logs = root.join("logs");
        fs::create_dir(&logs).expect("create logs directory");
        symlink(&logs, root.join("linked-logs")).expect("create log directory symlink");

        let error = validate_log_file_path_under(&root, Path::new("linked-logs/aigosd.log"))
            .expect_err("symlinked log path must fail");
        assert!(error.to_string().contains("symbolic link"));

        fs::remove_dir_all(&root).expect("remove temporary config root");
    }
}
