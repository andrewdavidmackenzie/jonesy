//! Configuration for allow/deny rules on panic causes.
//!
//! Configuration is loaded in order of precedence (later overrides earlier):
//! 1. Application defaults (in code)
//! 2. Cargo.toml `[package.metadata.jonesy]` section
//! 3. `jonesy.toml` file in the project root
//! 4. `--config <path>` command line option
//!
//! Rules can be global or scoped to specific paths/functions:
//! - Global rules apply to all panic points
//! - Scoped rules match by file path pattern or function name pattern
//! - More specific rules take precedence over less specific ones

use crate::panic_cause::PanicCause;
use glob::Pattern;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// A scoped rule that applies to specific paths or functions.
#[derive(Debug, Clone)]
pub struct ScopedRule {
    /// File path pattern (glob syntax, e.g., "**/tests/**")
    pub path: Option<Pattern>,
    /// Function name pattern (glob syntax, e.g., "my_crate::*::new")
    pub function: Option<Pattern>,
    /// Panic causes to allow (not report) when this rule matches
    pub allowed: HashSet<String>,
    /// Panic causes to deny (report) when this rule matches
    pub denied: HashSet<String>,
}

impl ScopedRule {
    /// Check if this rule matches the given file path and function name.
    /// Returns the specificity score if matched (higher = more specific), None if not matched.
    pub fn matches(&self, file_path: Option<&str>, function_name: Option<&str>) -> Option<u32> {
        let mut specificity = 0u32;

        // Check path pattern
        if let Some(ref pattern) = self.path {
            if let Some(path) = file_path {
                // Normalize path separators for matching
                let normalized = path.replace('\\', "/");
                if !pattern.matches(&normalized) {
                    return None;
                }
                // More specific path patterns get higher scores
                // Count non-wildcard characters as specificity
                specificity += pattern.as_str().chars().filter(|c| *c != '*').count() as u32;
            } else {
                return None;
            }
        }

        // Check function pattern
        if let Some(ref pattern) = self.function {
            if let Some(func) = function_name {
                if !pattern.matches(func) {
                    return None;
                }
                // Function patterns are more specific than path patterns
                specificity += 1000;
                specificity += pattern.as_str().chars().filter(|c| *c != '*').count() as u32;
            } else {
                return None;
            }
        }

        // If no patterns specified, rule doesn't match anything
        if self.path.is_none() && self.function.is_none() {
            return None;
        }

        Some(specificity)
    }

    /// Check if a cause is allowed by this rule.
    /// Returns Some(true) if allowed, Some(false) if denied, None if not specified.
    pub fn check_cause(&self, cause_id: &str) -> Option<bool> {
        // Check for wildcard
        if self.allowed.contains("*") {
            return Some(true);
        }
        if self.denied.contains("*") {
            return Some(false);
        }

        // Explicit deny takes precedence
        if self.denied.contains(cause_id) {
            return Some(false);
        }

        // Then check allow
        if self.allowed.contains(cause_id) {
            return Some(true);
        }

        // Rule doesn't specify this cause
        None
    }
}

/// TOML structure for a scoped rule
#[derive(Debug, Deserialize, Default)]
struct TomlScopedRule {
    /// File path pattern (glob syntax)
    #[serde(default)]
    path: Option<String>,
    /// Function name pattern (glob syntax)
    #[serde(default)]
    function: Option<String>,
    /// Panic causes to allow
    #[serde(default)]
    allow: Vec<String>,
    /// Panic causes to deny
    #[serde(default)]
    deny: Vec<String>,
}

/// Configuration for which panic causes to allow or deny.
#[derive(Debug, Clone)]
pub struct Config {
    /// Panic cause IDs that are allowed globally (not reported)
    allowed: HashSet<String>,
    /// Panic cause IDs that are denied globally (reported)
    denied: HashSet<String>,
    /// Scoped rules for path/function-specific allow/deny
    rules: Vec<ScopedRule>,
}

/// TOML configuration structure for jonesy
#[derive(Debug, Deserialize, Default)]
struct TomlConfig {
    /// Panic causes to allow (not report) globally
    #[serde(default)]
    allow: Vec<String>,
    /// Panic causes to deny (report) globally
    #[serde(default)]
    deny: Vec<String>,
    /// Scoped rules
    #[serde(default)]
    rules: Vec<TomlScopedRule>,
}

/// Cargo.toml package metadata structure
#[derive(Debug, Deserialize, Default)]
struct CargoMetadata {
    #[serde(default)]
    jonesy: Option<TomlConfig>,
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
            rules: Vec::new(),
        }
    }

    /// Check if a panic cause should be reported (is denied).
    /// This is the simple version that only checks global rules.
    /// Returns true if the panic should be shown in output.
    pub fn is_denied(&self, cause: &PanicCause) -> bool {
        self.is_denied_at(cause, None, None)
    }

    /// Check if a panic cause should be reported (is denied) at a specific location.
    /// Checks scoped rules first (most specific wins), then falls back to global rules.
    ///
    /// # Arguments
    /// * `cause` - The panic cause to check
    /// * `file_path` - Optional file path where the panic occurs
    /// * `function_name` - Optional function name where the panic occurs
    ///
    /// # Returns
    /// `true` if the panic should be reported, `false` if it should be allowed
    pub fn is_denied_at(
        &self,
        cause: &PanicCause,
        file_path: Option<&str>,
        function_name: Option<&str>,
    ) -> bool {
        let id = cause.id();

        // Find all matching scoped rules and sort by specificity
        let mut matching_rules: Vec<(u32, &ScopedRule)> = self
            .rules
            .iter()
            .filter_map(|rule| rule.matches(file_path, function_name).map(|s| (s, rule)))
            .collect();

        // Sort by specificity (highest first)
        matching_rules.sort_by(|a, b| b.0.cmp(&a.0));

        // Check scoped rules in order of specificity
        for (_, rule) in matching_rules {
            if let Some(allowed) = rule.check_cause(id) {
                return !allowed;
            }
        }

        // Fall back to global rules
        // Explicit deny takes precedence
        if self.denied.contains(id) {
            return true;
        }

        // An explicit "allow" means not denied
        if self.allowed.contains(id) {
            return false;
        }

        // Default: deny (report) unless explicitly allowed
        true
    }

    /// Validate a cause ID and return whether it's valid.
    fn is_valid_cause_id(id: &str) -> bool {
        id == "*" || PanicCause::all_ids().contains(&id)
    }

    /// Apply a TOML configuration, overriding current settings.
    fn apply_toml_config(&mut self, config: &TomlConfig) {
        // Validate and apply the global allow list
        for id in &config.allow {
            if Self::is_valid_cause_id(id) {
                self.allowed.insert(id.clone());
                self.denied.remove(id);
            } else {
                eprintln!("Warning: Unknown panic cause '{}' in allow list", id);
            }
        }

        // Validate and apply the global deny list
        for id in &config.deny {
            if Self::is_valid_cause_id(id) {
                self.denied.insert(id.clone());
                self.allowed.remove(id);
            } else {
                eprintln!("Warning: Unknown panic cause '{}' in deny list", id);
            }
        }

        // Parse and apply scoped rules
        for toml_rule in &config.rules {
            // Parse path pattern
            let path = toml_rule.path.as_ref().and_then(|p| match Pattern::new(p) {
                Ok(pattern) => Some(pattern),
                Err(e) => {
                    eprintln!("Warning: Invalid path pattern '{}': {}", p, e);
                    None
                }
            });

            // Parse function pattern
            let function = toml_rule
                .function
                .as_ref()
                .and_then(|f| match Pattern::new(f) {
                    Ok(pattern) => Some(pattern),
                    Err(e) => {
                        eprintln!("Warning: Invalid function pattern '{}': {}", f, e);
                        None
                    }
                });

            // Skip rules with no valid patterns
            if path.is_none() && function.is_none() {
                eprintln!("Warning: Scoped rule has no valid path or function pattern, skipping");
                continue;
            }

            // Validate and collect allowed causes
            let mut allowed = HashSet::new();
            for id in &toml_rule.allow {
                if Self::is_valid_cause_id(id) {
                    allowed.insert(id.clone());
                } else {
                    eprintln!("Warning: Unknown panic cause '{}' in scoped allow list", id);
                }
            }

            // Validate and collect denied causes
            let mut denied = HashSet::new();
            for id in &toml_rule.deny {
                if Self::is_valid_cause_id(id) {
                    denied.insert(id.clone());
                } else {
                    eprintln!("Warning: Unknown panic cause '{}' in scoped deny list", id);
                }
            }

            self.rules.push(ScopedRule {
                path,
                function,
                allowed,
                denied,
            });
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
            && let Some(jones_config) = metadata.jonesy
        {
            self.apply_toml_config(&jones_config);
        }
    }

    /// Load configuration from a jonesy.toml file.
    pub fn load_from_jones_toml(&mut self, jones_toml_path: &Path) {
        match fs::read_to_string(jones_toml_path) {
            Ok(content) => match toml::from_str::<TomlConfig>(&content) {
                Ok(config) => self.apply_toml_config(&config),
                Err(e) => eprintln!(
                    "Warning: Failed to parse {}: {}",
                    jones_toml_path.display(),
                    e
                ),
            },
            Err(e) => eprintln!(
                "Warning: Failed to read {}: {}",
                jones_toml_path.display(),
                e
            ),
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

    /// Load the full configuration by searching for config files.
    /// Looks for Cargo.toml and jonesy.toml starting from the given directory.
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

        // Look for jonesy.toml
        let jones_toml = project_dir.join("jonesy.toml");
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

        // Drop and unwind panics should be allowed by default
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
            rules: vec![],
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
            rules: vec![],
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

    #[test]
    fn test_scoped_rule_path_matching() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                path: Some("**/tests/**".to_string()),
                function: None,
                allow: vec!["unwrap".to_string(), "panic".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // In tests directory, unwrap should be allowed
        assert!(!config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/tests/test_main.rs"),
            None
        ));
        assert!(!config.is_denied_at(
            &PanicCause::ExplicitPanic,
            Some("src/tests/test_main.rs"),
            None
        ));

        // Outside tests, unwrap should be denied (global default)
        assert!(config.is_denied_at(&PanicCause::UnwrapNone, Some("src/main.rs"), None));
    }

    #[test]
    fn test_scoped_rule_function_matching() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                path: None,
                function: Some("my_crate::config::*".to_string()),
                allow: vec!["unwrap".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // In matching function, unwrap should be allowed
        assert!(!config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/config.rs"),
            Some("my_crate::config::load")
        ));

        // In non-matching function, unwrap should be denied
        assert!(config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/main.rs"),
            Some("my_crate::main")
        ));
    }

    #[test]
    fn test_scoped_rule_wildcard_allow() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                path: Some("tests/**".to_string()),
                function: None,
                allow: vec!["*".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // In tests, all panics should be allowed
        assert!(!config.is_denied_at(&PanicCause::UnwrapNone, Some("tests/integration.rs"), None));
        assert!(!config.is_denied_at(&PanicCause::BoundsCheck, Some("tests/integration.rs"), None));
        assert!(!config.is_denied_at(
            &PanicCause::ExplicitPanic,
            Some("tests/integration.rs"),
            None
        ));
    }

    #[test]
    fn test_scoped_rule_specificity() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![
                // Less specific: allow unwrap in all tests
                TomlScopedRule {
                    path: Some("**/tests/**".to_string()),
                    function: None,
                    allow: vec!["unwrap".to_string()],
                    deny: vec![],
                },
                // More specific: deny unwrap in specific test file
                TomlScopedRule {
                    path: Some("src/tests/strict_test.rs".to_string()),
                    function: None,
                    allow: vec![],
                    deny: vec!["unwrap".to_string()],
                },
            ],
        };
        config.apply_toml_config(&toml_config);

        // In general tests, unwrap allowed
        assert!(!config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/tests/normal_test.rs"),
            None
        ));

        // In strict_test.rs, unwrap denied (more specific rule)
        assert!(config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/tests/strict_test.rs"),
            None
        ));
    }

    #[test]
    fn test_function_rule_more_specific_than_path() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![
                // Path rule: deny unwrap in src
                TomlScopedRule {
                    path: Some("src/**".to_string()),
                    function: None,
                    allow: vec![],
                    deny: vec!["unwrap".to_string()],
                },
                // Function rule: allow unwrap in specific function
                TomlScopedRule {
                    path: None,
                    function: Some("my_crate::config::load".to_string()),
                    allow: vec!["unwrap".to_string()],
                    deny: vec![],
                },
            ],
        };
        config.apply_toml_config(&toml_config);

        // In src without function match, unwrap denied
        assert!(config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/main.rs"),
            Some("my_crate::main")
        ));

        // In src with function match, unwrap allowed (function rule more specific)
        assert!(!config.is_denied_at(
            &PanicCause::UnwrapNone,
            Some("src/config.rs"),
            Some("my_crate::config::load")
        ));
    }
}
