//! Configuration for allow/deny rules on panic causes.
//!
//! Configuration is loaded in order of precedence (later overrides earlier):
//! 1. Application defaults (in code)
//! 2. Cargo.toml `[package.metadata.jones]` section
//! 3. `jones.toml` file in project root
//! 4. `--config <path>` command line option

use crate::panic_cause::PanicCause;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Configuration for which panic causes to allow or deny.
#[derive(Debug, Clone)]
pub struct Config {
    /// Panic cause IDs that are allowed (not reported)
    allowed: HashSet<String>,
    /// Panic cause IDs that are denied (reported)
    denied: HashSet<String>,
}

/// TOML configuration structure for jones
#[derive(Debug, Deserialize, Default)]
struct TomlConfig {
    /// Panic causes to allow (not report)
    #[serde(default)]
    allow: Vec<String>,
    /// Panic causes to deny (report)
    #[serde(default)]
    deny: Vec<String>,
}

/// Cargo.toml package metadata structure
#[derive(Debug, Deserialize, Default)]
struct CargoMetadata {
    #[serde(default)]
    jones: Option<TomlConfig>,
}

/// Cargo.toml package structure
#[derive(Debug, Deserialize, Default)]
struct CargoPackage {
    #[serde(default)]
    metadata: Option<CargoMetadata>,
}

/// Cargo.toml structure (partial)
#[derive(Debug, Deserialize, Default)]
struct CargoToml {
    #[serde(default)]
    package: Option<CargoPackage>,
}

impl Default for Config {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl Config {
    /// Create a new config with application defaults.
    /// By default, drop and unwind panics are allowed (not reported),
    /// and all other panics are denied (reported).
    pub fn with_defaults() -> Self {
        let mut allowed = HashSet::new();
        // Drop/cleanup panics are allowed by default
        allowed.insert("drop".to_string());
        allowed.insert("unwind".to_string());

        Config {
            allowed,
            denied: HashSet::new(),
        }
    }

    /// Check if a panic cause should be reported (is denied).
    /// Returns true if the panic should be shown in output.
    pub fn is_denied(&self, cause: &PanicCause) -> bool {
        let id = cause.id();

        // Explicit deny takes precedence
        if self.denied.contains(id) {
            return true;
        }

        // Explicit allow means not denied
        if self.allowed.contains(id) {
            return false;
        }

        // Default: deny (report) unless explicitly allowed
        true
    }

    /// Apply a TOML configuration, overriding current settings.
    fn apply_toml_config(&mut self, config: &TomlConfig) {
        // Validate and apply allow list
        for id in &config.allow {
            if PanicCause::all_ids().contains(&id.as_str()) {
                self.allowed.insert(id.clone());
                self.denied.remove(id);
            } else {
                eprintln!("Warning: Unknown panic cause '{}' in allow list", id);
            }
        }

        // Validate and apply deny list
        for id in &config.deny {
            if PanicCause::all_ids().contains(&id.as_str()) {
                self.denied.insert(id.clone());
                self.allowed.remove(id);
            } else {
                eprintln!("Warning: Unknown panic cause '{}' in deny list", id);
            }
        }
    }

    /// Load configuration from Cargo.toml metadata.
    /// Reports parse errors but continues with defaults.
    pub fn load_from_cargo_toml(&mut self, cargo_toml_path: &Path) {
        let content = match fs::read_to_string(cargo_toml_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to read {}: {}",
                    cargo_toml_path.display(),
                    e
                );
                return;
            }
        };

        let cargo: CargoToml = match toml::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "Warning: Failed to parse {}: {}",
                    cargo_toml_path.display(),
                    e
                );
                return;
            }
        };

        if let Some(package) = cargo.package
            && let Some(metadata) = package.metadata
            && let Some(jones_config) = metadata.jones
        {
            self.apply_toml_config(&jones_config);
        }
    }

    /// Load configuration from a jones.toml file.
    pub fn load_from_jones_toml(&mut self, jones_toml_path: &Path) {
        if let Ok(content) = fs::read_to_string(jones_toml_path) {
            match toml::from_str::<TomlConfig>(&content) {
                Ok(config) => self.apply_toml_config(&config),
                Err(e) => eprintln!(
                    "Warning: Failed to parse {}: {}",
                    jones_toml_path.display(),
                    e
                ),
            }
        }
    }

    /// Load configuration from a custom config file path.
    /// Returns an error if the file cannot be read or parsed.
    /// This is used for explicit --config overrides where failures should be fatal.
    pub fn load_from_config_file(&mut self, config_path: &Path) -> Result<(), String> {
        if !config_path.exists() {
            return Err(format!("Config file not found: {}", config_path.display()));
        }

        let content = fs::read_to_string(config_path)
            .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;

        let config: TomlConfig = toml::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;

        self.apply_toml_config(&config);
        Ok(())
    }

    /// Load full configuration by searching for config files.
    /// Looks for Cargo.toml and jones.toml starting from the given directory.
    /// Returns an error if an explicit config_override is provided and fails to load.
    pub fn load_for_project(
        project_dir: &Path,
        config_override: Option<&Path>,
    ) -> Result<Self, String> {
        let mut config = Self::with_defaults();

        // Look for Cargo.toml
        let cargo_toml = project_dir.join("Cargo.toml");
        if cargo_toml.exists() {
            config.load_from_cargo_toml(&cargo_toml);
        }

        // Look for jones.toml
        let jones_toml = project_dir.join("jones.toml");
        if jones_toml.exists() {
            config.load_from_jones_toml(&jones_toml);
        }

        // Apply command-line config override if provided (failures are fatal)
        if let Some(config_path) = config_override {
            config.load_from_config_file(config_path)?;
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::with_defaults();

        // Drop panics should be allowed by default
        assert!(!config.is_denied(&PanicCause::PanicInDrop));
        assert!(!config.is_denied(&PanicCause::CannotUnwind));

        // Other panics should be denied by default
        assert!(config.is_denied(&PanicCause::ExplicitPanic));
        assert!(config.is_denied(&PanicCause::BoundsCheck));
        assert!(config.is_denied(&PanicCause::UnwrapNone));
    }

    #[test]
    fn test_toml_config_deny_drop() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec!["drop".to_string()],
        };
        config.apply_toml_config(&toml_config);

        // Drop should now be denied
        assert!(config.is_denied(&PanicCause::PanicInDrop));
    }

    #[test]
    fn test_toml_config_allow_panic() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec!["panic".to_string()],
            deny: vec![],
        };
        config.apply_toml_config(&toml_config);

        // Explicit panic should now be allowed
        assert!(!config.is_denied(&PanicCause::ExplicitPanic));
    }

    #[test]
    fn test_panic_cause_ids() {
        // Verify all PanicCause variants have valid IDs
        assert_eq!(PanicCause::ExplicitPanic.id(), "panic");
        assert_eq!(PanicCause::BoundsCheck.id(), "bounds");
        assert_eq!(PanicCause::PanicInDrop.id(), "drop");
    }
}
