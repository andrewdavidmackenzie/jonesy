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
    ///
    /// Priority order:
    /// 1. Explicit deny of specific cause
    /// 2. Explicit allow of specific cause
    /// 3. Wildcard deny
    /// 4. Wildcard allow
    pub fn check_cause(&self, cause_id: &str) -> Option<bool> {
        // Check explicit entries first (before wildcards)
        // Explicit deny takes precedence over explicit allow
        if self.denied.contains(cause_id) {
            return Some(false);
        }
        if self.allowed.contains(cause_id) {
            return Some(true);
        }

        // Then check wildcards
        if self.denied.contains("*") {
            return Some(false);
        }
        if self.allowed.contains("*") {
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
    ///
    /// For scoped rules support, use `is_denied_at()` with location info.
    #[allow(dead_code)] // Part of public API, used in tests
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
        // Track rule index for tiebreaking (later rules win when specificity is equal)
        let mut matching_rules: Vec<(u32, usize, &ScopedRule)> = self
            .rules
            .iter()
            .enumerate()
            .filter_map(|(idx, rule)| {
                rule.matches(file_path, function_name)
                    .map(|specificity| (specificity, idx, rule))
            })
            .collect();

        // Sort by specificity first, then by declaration order (later wins)
        matching_rules.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

        // Get the parent ID (e.g., "overflow" for "div_overflow")
        let parent_id = cause.parent_id();

        // Check scoped rules in order of specificity
        for (_, _, rule) in matching_rules {
            // Check specific ID first, then parent ID
            if let Some(allowed) = rule.check_cause(id) {
                return !allowed;
            }
            if let Some(parent) = parent_id {
                if let Some(allowed) = rule.check_cause(parent) {
                    return !allowed;
                }
            }
        }

        // Fall back to global rules
        // Check explicit entries first, then wildcards (same priority as scoped rules)

        // Explicit deny takes precedence (check specific first, then parent)
        if self.denied.contains(id) {
            return true;
        }
        if let Some(parent) = parent_id {
            if self.denied.contains(parent) {
                return true;
            }
        }

        // An explicit "allow" means not denied (check specific first, then parent)
        if self.allowed.contains(id) {
            return false;
        }
        if let Some(parent) = parent_id {
            if self.allowed.contains(parent) {
                return false;
            }
        }

        // Then check global wildcards
        if self.denied.contains("*") {
            return true;
        }
        if self.allowed.contains("*") {
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

            let has_path = toml_rule.path.is_some();
            let has_function = toml_rule.function.is_some();

            // Rules that genuinely omitted both selectors are treated as global rules.
            // If a selector was provided but failed to parse (invalid glob), skip the
            // rule entirely — the warning was already printed above.
            if !has_path && !has_function {
                for id in &toml_rule.allow {
                    if Self::is_valid_cause_id(id) {
                        self.allowed.insert(id.clone());
                        self.denied.remove(id);
                    } else {
                        eprintln!("Warning: Unknown panic cause '{}' in allow list", id);
                    }
                }
                for id in &toml_rule.deny {
                    if Self::is_valid_cause_id(id) {
                        self.denied.insert(id.clone());
                        self.allowed.remove(id);
                    } else {
                        eprintln!("Warning: Unknown panic cause '{}' in deny list", id);
                    }
                }
                continue;
            }

            // Skip rules where a selector was specified but failed to parse
            if path.is_none() && function.is_none() {
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
        assert!(config.is_denied(&PanicCause::Unwrap));
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
    fn test_rules_without_pattern_treated_as_global() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                path: None,
                function: None,
                allow: vec!["expect".to_string(), "capacity".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // expect and capacity should be globally allowed
        assert!(!config.is_denied(&PanicCause::ExpectNone));
        assert!(!config.is_denied(&PanicCause::ExpectErr));
        assert!(!config.is_denied(&PanicCause::CapacityOverflow));

        // Other causes should still be denied
        assert!(config.is_denied(&PanicCause::Unwrap));
        assert!(config.is_denied(&PanicCause::ExplicitPanic));
    }

    #[test]
    fn test_malformed_scoped_rule_not_promoted_to_global() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                // Invalid glob pattern — should NOT become a global rule
                path: Some("[".to_string()),
                function: None,
                allow: vec!["unwrap".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // unwrap should still be denied — the malformed rule should be skipped
        assert!(config.is_denied(&PanicCause::Unwrap));
    }

    #[test]
    fn test_scoped_rule_allows_format_on_function() {
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                path: None,
                function: Some("alloc::fmt::format".to_string()),
                allow: vec!["format".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // format should be allowed when called from alloc::fmt::format
        assert!(!config.is_denied_at(
            &PanicCause::FormattingError,
            Some("src/main.rs"),
            Some("alloc::fmt::format"),
        ));

        // format should still be denied for other functions
        assert!(config.is_denied_at(
            &PanicCause::FormattingError,
            Some("src/main.rs"),
            Some("my_crate::render"),
        ));
    }

    #[test]
    fn test_meshchat_jonesy_toml_parsed_correctly() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let toml_path = dir.path().join("jonesy.toml");
        let mut f = std::fs::File::create(&toml_path).unwrap();
        write!(
            f,
            "[[rules]]\nallow = [\"expect\", \"capacity\"]\n[[rules]]\nfunction = \"alloc::fmt::format\"\nallow = [\"format\"]\n"
        )
        .unwrap();

        let mut config = Config::with_defaults();
        config.load_from_jones_toml(&toml_path);

        // First rule (no path/function) should be treated as global
        assert!(!config.is_denied(&PanicCause::ExpectNone));
        assert!(!config.is_denied(&PanicCause::ExpectErr));
        assert!(!config.is_denied(&PanicCause::CapacityOverflow));

        // Second rule should allow format on alloc::fmt::format
        assert!(!config.is_denied_at(
            &PanicCause::FormattingError,
            Some("src/main.rs"),
            Some("alloc::fmt::format"),
        ));

        // format should still be denied for other functions
        assert!(config.is_denied_at(
            &PanicCause::FormattingError,
            Some("src/main.rs"),
            Some("my_crate::render"),
        ));

        // Other causes should still be denied
        assert!(config.is_denied(&PanicCause::Unwrap));
        assert!(config.is_denied(&PanicCause::ExplicitPanic));
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
        assert!(!config.is_denied_at(&PanicCause::Unwrap, Some("src/tests/test_main.rs"), None));
        assert!(!config.is_denied_at(
            &PanicCause::ExplicitPanic,
            Some("src/tests/test_main.rs"),
            None
        ));

        // Outside tests, unwrap should be denied (global default)
        assert!(config.is_denied_at(&PanicCause::Unwrap, Some("src/main.rs"), None));
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
            &PanicCause::Unwrap,
            Some("src/config.rs"),
            Some("my_crate::config::load")
        ));

        // In non-matching function, unwrap should be denied
        assert!(config.is_denied_at(
            &PanicCause::Unwrap,
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
        assert!(!config.is_denied_at(&PanicCause::Unwrap, Some("tests/integration.rs"), None));
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
        assert!(!config.is_denied_at(&PanicCause::Unwrap, Some("src/tests/normal_test.rs"), None));

        // In strict_test.rs, unwrap denied (more specific rule)
        assert!(config.is_denied_at(&PanicCause::Unwrap, Some("src/tests/strict_test.rs"), None));
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
            &PanicCause::Unwrap,
            Some("src/main.rs"),
            Some("my_crate::main")
        ));

        // In src with function match, unwrap allowed (function rule more specific)
        assert!(!config.is_denied_at(
            &PanicCause::Unwrap,
            Some("src/config.rs"),
            Some("my_crate::config::load")
        ));
    }

    #[test]
    fn test_overflow_parent_id_allows_all_overflow() {
        // allow = ["overflow"] should match div_overflow, rem_overflow, shift_overflow
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec!["overflow".to_string()],
            deny: vec![],
            rules: vec![],
        };
        config.apply_toml_config(&toml_config);

        // Division overflow should be allowed via parent "overflow"
        assert!(!config.is_denied(&PanicCause::ArithmeticOverflow("division".to_string())));
        // Remainder overflow should be allowed via parent "overflow"
        assert!(!config.is_denied(&PanicCause::ArithmeticOverflow("remainder".to_string())));
        // Shift overflow should be allowed via parent "overflow"
        assert!(!config.is_denied(&PanicCause::ShiftOverflow("left".to_string())));
        // Generic overflow (add/sub/mul) should be allowed directly
        assert!(!config.is_denied(&PanicCause::ArithmeticOverflow("addition".to_string())));
    }

    #[test]
    fn test_assert_allow_config() {
        // allow = ["assert"] should match AssertFailed
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec!["assert".to_string()],
            deny: vec![],
            rules: vec![],
        };
        config.apply_toml_config(&toml_config);

        assert!(!config.is_denied(&PanicCause::AssertFailed));
        // Other causes should still be denied
        assert!(config.is_denied(&PanicCause::Unwrap));
    }

    #[test]
    fn test_specific_div_overflow_id() {
        // allow = ["div_overflow"] should only match division overflow, not others
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec!["div_overflow".to_string()],
            deny: vec![],
            rules: vec![],
        };
        config.apply_toml_config(&toml_config);

        // Division overflow should be allowed
        assert!(!config.is_denied(&PanicCause::ArithmeticOverflow("division".to_string())));
        // Remainder overflow should NOT be allowed (different specific ID)
        assert!(config.is_denied(&PanicCause::ArithmeticOverflow("remainder".to_string())));
        // Shift overflow should NOT be allowed
        assert!(config.is_denied(&PanicCause::ShiftOverflow("left".to_string())));
        // Generic overflow should NOT be allowed
        assert!(config.is_denied(&PanicCause::ArithmeticOverflow("addition".to_string())));
    }

    #[test]
    fn test_div_zero_independent_of_overflow() {
        // div_zero is separate from overflow
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec!["overflow".to_string()],
            deny: vec![],
            rules: vec![],
        };
        config.apply_toml_config(&toml_config);

        // Division by zero should NOT be allowed (separate cause)
        assert!(config.is_denied(&PanicCause::DivisionByZero));
    }

    #[test]
    fn test_scoped_rule_with_overflow_parent() {
        // Scoped rules should also respect parent IDs
        let mut config = Config::with_defaults();
        let toml_config = TomlConfig {
            allow: vec![],
            deny: vec![],
            rules: vec![TomlScopedRule {
                path: Some("src/math/**".to_string()),
                function: None,
                allow: vec!["overflow".to_string()],
                deny: vec![],
            }],
        };
        config.apply_toml_config(&toml_config);

        // In math module, all overflow types should be allowed
        assert!(!config.is_denied_at(
            &PanicCause::ArithmeticOverflow("division".to_string()),
            Some("src/math/ops.rs"),
            None
        ));
        assert!(!config.is_denied_at(
            &PanicCause::ShiftOverflow("right".to_string()),
            Some("src/math/bits.rs"),
            None
        ));

        // Outside math module, overflow should still be denied
        assert!(config.is_denied_at(
            &PanicCause::ArithmeticOverflow("division".to_string()),
            Some("src/main.rs"),
            None
        ));
    }
}
