use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::collections::HashSet;
use indexmap::IndexMap;

use aigos::CANONICAL_LAYERS;

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub meshes: IndexMap<String, MeshConfig>,
    pub options: Options,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MeshConfig {
    pub layers: Vec<String>,
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
    let valid: HashSet<&str> = CANONICAL_LAYERS.iter().copied().collect();
    
    if config.meshes.is_empty() {
        return Err("Config must define at least one mesh".into());
    }
    
    for (mesh_name, mesh_cfg) in &config.meshes {
        if mesh_cfg.layers.is_empty() {
            return Err(format!("Mesh '{}' has no layers", mesh_name).into());
        }
        for layer in &mesh_cfg.layers {
            if !valid.contains(layer.as_str()) {
                return Err(format!("Invalid layer '{}' in mesh '{}'", layer, mesh_name).into());
            }
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