use indexmap::IndexMap;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::Path;

use aigos::{is_core_layer, is_extension_layer, is_valid_layer, CANONICAL_CORE_LAYERS};

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
                "meshes" => {
                    meshes = Some(map.next_value_seed(MeshesSeed)?);
                }
                "options" => {
                    options = Some(map.next_value()?);
                }
                _ => {
                    let _ = map.next_value::<IgnoredAny>()?;
                }
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
pub struct MeshConfig {
    #[serde(default)]
    pub layers: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug)]
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
