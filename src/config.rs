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
    /// Public base URL of the instance, e.g. "https://albo.example.com", with
    /// no trailing slash. Used to build absolute URLs for Open Graph / social
    /// card tags, which require fully-qualified links. When empty, the og:url
    /// and og:image tags are omitted (title/description still render).
    #[serde(default)]
    pub base_url: String,
}

impl Directory {
    /// Join the base URL with a root-relative path into an absolute URL, or
    /// return None when no base URL is configured. `path` should start with
    /// '/'. Trailing slashes on the base are trimmed to avoid '//'.
    pub fn abs_url(&self, path: &str) -> Option<String> {
        let base = self.base_url.trim_end_matches('/');
        if base.is_empty() {
            return None;
        }
        Some(format!("{base}{path}"))
    }
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
    fn abs_url_joins_and_handles_missing_base() {
        let d = Directory {
            name: "x".into(),
            entity: "a".into(),
            entities: "as".into(),
            tagline: String::new(),
            base_url: "https://albo.example.com".into(),
        };
        assert_eq!(
            d.abs_url("/a/jane").as_deref(),
            Some("https://albo.example.com/a/jane")
        );
        // Trailing slash on the base is trimmed, no double slash.
        let trailing = Directory {
            base_url: "https://albo.example.com/".into(),
            ..Directory {
                name: "x".into(),
                entity: "a".into(),
                entities: "as".into(),
                tagline: String::new(),
                base_url: String::new(),
            }
        };
        assert_eq!(
            trailing.abs_url("/").as_deref(),
            Some("https://albo.example.com/")
        );
        // No base configured -> None (tags omitted, no crash).
        let empty = Directory {
            base_url: String::new(),
            ..trailing
        };
        assert_eq!(empty.abs_url("/a/jane"), None);
    }

    #[test]
    fn example_config_parses() {
        let example = std::path::Path::new("directory.example.toml");
        let config = Config::load(example).expect("example config must always parse");
        assert_eq!(config.directory.entity, "tattooer");
        assert!(!config.tags.available.is_empty());
    }
}
