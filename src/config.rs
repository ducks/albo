//! Instance configuration. One albo process serves one directory; this file
//! is what makes an instance specific (name, entity label, tags, theme).
//! The engine must contain no directory-specific assumptions - the lemma
//! rule: the product is generic, the instance is data.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub directory: Directory,
    #[serde(default)]
    pub tags: Tags,
    pub server: Server,
}

#[derive(Debug, Deserialize)]
pub struct Directory {
    /// The instance's brand, e.g. "Meat Ledger".
    pub name: String,
    /// What one entry is called, e.g. "tattooer".
    pub entity: String,
    /// Plural, e.g. "tattooers".
    pub entities: String,
    #[serde(default)]
    pub tagline: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Tags {
    /// The taxonomy admins can assign. Order is display order.
    #[serde(default)]
    pub available: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Server {
    pub bind: String,
    pub database: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read config at {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let example = std::path::Path::new("directory.example.toml");
        let config = Config::load(example).expect("example config must always parse");
        assert_eq!(config.directory.entity, "tattooer");
        assert!(!config.tags.available.is_empty());
    }
}
