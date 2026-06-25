use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,

    #[serde(default)]
    pub apps: HashMap<String, TrackedApp>,

    /// Custom config path (not serialized). When set, overrides the default location.
    #[serde(skip)]
    pub config_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Auto-approve behavior for package installations
    #[serde(default)]
    pub auto_approve: AutoApprove,
    /// Default package manager for new apps
    #[serde(default)]
    pub default_package_manager: PackageManagerType,
    /// Hide package manager output during install
    #[serde(default)]
    pub quiet_package_manager: bool,
    /// Path to log file (if set, enables file logging)
    pub log_file: Option<String>,
    /// Log level for file logging (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_approve: AutoApprove::default(),
            default_package_manager: PackageManagerType::default(),
            quiet_package_manager: false,
            log_file: None,
            log_level: default_log_level(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AutoApprove {
    /// Always auto-approve installs (zypper -y)
    Always,
    /// Auto-approve only when no new dependencies are pulled in
    #[default]
    NoDeps,
    /// Never auto-approve — always prompt
    Never,
}

impl std::fmt::Display for AutoApprove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutoApprove::Always => write!(f, "always"),
            AutoApprove::NoDeps => write!(f, "no_deps"),
            AutoApprove::Never => write!(f, "never"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedApp {
    /// GitHub repository in "owner/repo" format
    pub repo: String,
    /// Glob pattern to match the desired release asset (e.g. "*.x86_64.rpm")
    pub asset_pattern: String,
    /// Package manager to use for installation
    #[serde(default = "default_package_manager")]
    pub package_manager: PackageManagerType,
    /// Currently installed version (if any)
    pub installed_version: Option<String>,
    /// When the app was last checked for updates
    pub last_checked: Option<String>,
    /// Whether to include prerelease versions
    #[serde(default)]
    pub allow_prerelease: bool,
    /// Version constraint for pinning (e.g. "1.0.24", "1.*", ">=2.0.0,<3.0.0")
    pub version_pin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PackageManagerType {
    #[default]
    Zypper,
    Dnf,
    Apt,
    Pacman,
}

impl std::fmt::Display for PackageManagerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageManagerType::Zypper => write!(f, "zypper"),
            PackageManagerType::Dnf => write!(f, "dnf"),
            PackageManagerType::Apt => write!(f, "apt"),
            PackageManagerType::Pacman => write!(f, "pacman"),
        }
    }
}

impl std::str::FromStr for PackageManagerType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "zypper" => Ok(PackageManagerType::Zypper),
            "dnf" => Ok(PackageManagerType::Dnf),
            "apt" | "apt-get" => Ok(PackageManagerType::Apt),
            "pacman" => Ok(PackageManagerType::Pacman),
            other => anyhow::bail!(
                "Unsupported package manager: '{other}'. Supported: zypper, dnf, apt, pacman"
            ),
        }
    }
}

fn default_package_manager() -> PackageManagerType {
    PackageManagerType::Zypper
}

/// Parse and evaluate version constraints for pinning.
///
/// Supported formats:
/// - Exact: `1.0.24` — only this version
/// - Wildcard prefix: `1.*`, `1.0.*` — any version matching the prefix
/// - Semver range: `>=1.0.0`, `>=1.0.0,<2.0.0`, `^1.0`, `~1.2` — standard semver ranges
pub fn version_matches_pin(version: &str, pin: &str) -> bool {
    let clean = version.trim_start_matches('v');
    let clean_pin = pin.trim_start_matches('v');

    // Wildcard prefix (e.g. "1.*", "1.0.*")
    if clean_pin.contains('*') {
        let prefix = clean_pin.trim_end_matches(".*").trim_end_matches("*");
        let prefix = prefix.trim_end_matches('.');
        if prefix.is_empty() {
            return true; // "*" matches everything
        }
        // Version must start with the prefix followed by a dot or end
        return clean == prefix || clean.starts_with(&format!("{prefix}."));
    }

    // Semver range operators (>=, <=, >, <, ^, ~, or comma-separated)
    if clean_pin.starts_with('>')
        || clean_pin.starts_with('<')
        || clean_pin.starts_with('^')
        || clean_pin.starts_with('~')
        || clean_pin.contains(',')
    {
        if let Ok(req) = semver::VersionReq::parse(clean_pin) {
            // Try parsing the version, padding with .0 if needed
            if let Some(ver) = parse_semver_loose(clean) {
                return req.matches(&ver);
            }
        }
        return false;
    }

    // Exact match
    clean == clean_pin
}

/// Loosely parse a version string into a semver::Version, padding with .0 as needed.
fn parse_semver_loose(s: &str) -> Option<semver::Version> {
    let clean = s.trim_start_matches('v');
    if let Ok(v) = semver::Version::parse(clean) {
        return Some(v);
    }
    // Try padding: "1" -> "1.0.0", "1.2" -> "1.2.0"
    let parts: Vec<&str> = clean.splitn(3, '.').collect();
    match parts.len() {
        1 => semver::Version::parse(&format!("{}.0.0", parts[0])).ok(),
        2 => semver::Version::parse(&format!("{}.{}.0", parts[0], parts[1])).ok(),
        _ => None,
    }
}

impl Config {
    pub fn default_config_dir() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("galvaan");
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn default_config_path() -> Result<PathBuf> {
        Ok(Self::default_config_dir()?.join("config.toml"))
    }

    /// Resolve the config file path (custom override or default)
    fn resolve_path(&self) -> Result<PathBuf> {
        match &self.config_file {
            Some(p) => Ok(p.clone()),
            None => Self::default_config_path(),
        }
    }

    pub fn load() -> Result<Self> {
        Self::load_from(Self::default_config_path()?)
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                config_file: Some(path),
                ..Default::default()
            });
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config at {}", path.display()))?;
        config.config_file = Some(path);
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = self.resolve_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write config to {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_config() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        (dir, path)
    }

    #[test]
    fn test_load_missing_config_returns_default() {
        let (_dir, path) = temp_config();
        let config = Config::load_from(path).unwrap();
        assert!(config.apps.is_empty());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let (_dir, path) = temp_config();
        let mut config = Config::load_from(path.clone()).unwrap();

        config.apps.insert(
            "test-app".to_string(),
            TrackedApp {
                repo: "owner/repo".to_string(),
                asset_pattern: "*.rpm".to_string(),
                package_manager: PackageManagerType::Zypper,
                installed_version: Some("1.0.0".to_string()),
                last_checked: None,
                allow_prerelease: false,
                version_pin: None,
            },
        );
        config.save().unwrap();

        let loaded = Config::load_from(path).unwrap();
        assert_eq!(loaded.apps.len(), 1);
        let app = loaded.apps.get("test-app").unwrap();
        assert_eq!(app.repo, "owner/repo");
        assert_eq!(app.asset_pattern, "*.rpm");
        assert_eq!(app.package_manager, PackageManagerType::Zypper);
        assert_eq!(app.installed_version.as_deref(), Some("1.0.0"));
        assert!(!app.allow_prerelease);
        assert!(app.version_pin.is_none());
    }

    #[test]
    fn test_add_and_remove_app() {
        let (_dir, path) = temp_config();
        let mut config = Config::load_from(path.clone()).unwrap();

        config.apps.insert(
            "app1".to_string(),
            TrackedApp {
                repo: "owner/app1".to_string(),
                asset_pattern: "*.rpm".to_string(),
                package_manager: PackageManagerType::Zypper,
                installed_version: None,
                last_checked: None,
                allow_prerelease: false,
                version_pin: None,
            },
        );
        config.apps.insert(
            "app2".to_string(),
            TrackedApp {
                repo: "owner/app2".to_string(),
                asset_pattern: "*.deb".to_string(),
                package_manager: PackageManagerType::Zypper,
                installed_version: None,
                last_checked: None,
                allow_prerelease: false,
                version_pin: None,
            },
        );
        config.save().unwrap();

        let mut loaded = Config::load_from(path.clone()).unwrap();
        assert_eq!(loaded.apps.len(), 2);

        loaded.apps.remove("app1");
        loaded.save().unwrap();

        let reloaded = Config::load_from(path).unwrap();
        assert_eq!(reloaded.apps.len(), 1);
        assert!(reloaded.apps.contains_key("app2"));
    }

    #[test]
    fn test_parse_toml_directly() {
        let toml_str = r#"
[apps.github-copilot]
repo = "github/app"
asset_pattern = "GitHub-Copilot-linux-x64.rpm"
package_manager = "zypper"
installed_version = "1.0.24"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let app = config.apps.get("github-copilot").unwrap();
        assert_eq!(app.repo, "github/app");
        assert_eq!(app.installed_version.as_deref(), Some("1.0.24"));
    }

    #[test]
    fn test_default_package_manager() {
        let toml_str = r#"
[apps.myapp]
repo = "owner/repo"
asset_pattern = "*.rpm"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let app = config.apps.get("myapp").unwrap();
        assert_eq!(app.package_manager, PackageManagerType::Zypper);
    }

    #[test]
    fn test_package_manager_display() {
        assert_eq!(format!("{}", PackageManagerType::Zypper), "zypper");
        assert_eq!(format!("{}", PackageManagerType::Dnf), "dnf");
        assert_eq!(format!("{}", PackageManagerType::Apt), "apt");
        assert_eq!(format!("{}", PackageManagerType::Pacman), "pacman");
    }

    #[test]
    fn test_default_settings() {
        let config = Config::default();
        assert_eq!(config.settings.auto_approve, AutoApprove::NoDeps);
        assert_eq!(
            config.settings.default_package_manager,
            PackageManagerType::Zypper
        );
        assert!(!config.settings.quiet_package_manager);
        assert!(config.settings.log_file.is_none());
        assert_eq!(config.settings.log_level, "info");
    }

    #[test]
    fn test_settings_roundtrip() {
        let (_dir, path) = temp_config();
        let mut config = Config::load_from(path.clone()).unwrap();

        config.settings.auto_approve = AutoApprove::Always;
        config.settings.quiet_package_manager = true;
        config.settings.log_file = Some("/tmp/galvaan.log".to_string());
        config.settings.log_level = "debug".to_string();
        config.save().unwrap();

        let loaded = Config::load_from(path).unwrap();
        assert_eq!(loaded.settings.auto_approve, AutoApprove::Always);
        assert!(loaded.settings.quiet_package_manager);
        assert_eq!(
            loaded.settings.log_file.as_deref(),
            Some("/tmp/galvaan.log")
        );
        assert_eq!(loaded.settings.log_level, "debug");
    }

    #[test]
    fn test_parse_settings_from_toml() {
        let toml_str = r#"
[settings]
auto_approve = "always"
default_package_manager = "zypper"
quiet_package_manager = true
log_file = "/var/log/galvaan.log"
log_level = "trace"

[apps.myapp]
repo = "owner/repo"
asset_pattern = "*.rpm"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.settings.auto_approve, AutoApprove::Always);
        assert_eq!(
            config.settings.default_package_manager,
            PackageManagerType::Zypper
        );
        assert!(config.settings.quiet_package_manager);
        assert_eq!(
            config.settings.log_file.as_deref(),
            Some("/var/log/galvaan.log")
        );
        assert_eq!(config.settings.log_level, "trace");
    }

    #[test]
    fn test_auto_approve_variants() {
        assert_eq!(format!("{}", AutoApprove::Always), "always");
        assert_eq!(format!("{}", AutoApprove::NoDeps), "no_deps");
        assert_eq!(format!("{}", AutoApprove::Never), "never");

        // Roundtrip through TOML config
        for (variant, label) in [
            (AutoApprove::Always, "always"),
            (AutoApprove::NoDeps, "no_deps"),
            (AutoApprove::Never, "never"),
        ] {
            let toml_str = format!("[settings]\nauto_approve = \"{label}\"\n[apps]\n");
            let config: Config = toml::from_str(&toml_str).unwrap();
            assert_eq!(config.settings.auto_approve, variant);
        }
    }

    #[test]
    fn test_config_missing_settings_uses_defaults() {
        let toml_str = r#"
[apps.myapp]
repo = "owner/repo"
asset_pattern = "*.rpm"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.settings.auto_approve, AutoApprove::NoDeps);
        assert_eq!(
            config.settings.default_package_manager,
            PackageManagerType::Zypper
        );
        assert!(!config.settings.quiet_package_manager);
        assert!(config.settings.log_file.is_none());
        assert_eq!(config.settings.log_level, "info");
    }

    #[test]
    fn test_package_manager_from_str() {
        assert_eq!(
            "zypper".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Zypper
        );
        assert_eq!(
            "ZYPPER".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Zypper
        );
        assert!("unknown".parse::<PackageManagerType>().is_err());
    }

    // ─── Version pin tests ───────────────────────────────────────────────────

    #[test]
    fn test_version_matches_pin_exact() {
        assert!(version_matches_pin("1.0.24", "1.0.24"));
        assert!(version_matches_pin("v1.0.24", "1.0.24"));
        assert!(version_matches_pin("1.0.24", "v1.0.24"));
        assert!(!version_matches_pin("1.0.25", "1.0.24"));
        assert!(!version_matches_pin("2.0.0", "1.0.24"));
    }

    #[test]
    fn test_version_matches_pin_wildcard() {
        assert!(version_matches_pin("1.0.24", "1.*"));
        assert!(version_matches_pin("1.5.0", "1.*"));
        assert!(version_matches_pin("1.0.0", "1.0.*"));
        assert!(version_matches_pin("1.0.99", "1.0.*"));
        assert!(!version_matches_pin("2.0.0", "1.*"));
        assert!(!version_matches_pin("1.1.0", "1.0.*"));
    }

    #[test]
    fn test_version_matches_pin_wildcard_star_only() {
        assert!(version_matches_pin("1.0.0", "*"));
        assert!(version_matches_pin("99.99.99", "*"));
    }

    #[test]
    fn test_version_matches_pin_semver_range() {
        assert!(version_matches_pin("1.5.0", ">=1.0.0,<2.0.0"));
        assert!(version_matches_pin("1.0.0", ">=1.0.0,<2.0.0"));
        assert!(!version_matches_pin("2.0.0", ">=1.0.0,<2.0.0"));
        assert!(!version_matches_pin("0.9.0", ">=1.0.0,<2.0.0"));
    }

    #[test]
    fn test_version_matches_pin_caret() {
        // ^1.0 means >=1.0.0, <2.0.0
        assert!(version_matches_pin("1.0.0", "^1.0"));
        assert!(version_matches_pin("1.9.9", "^1.0"));
        assert!(!version_matches_pin("2.0.0", "^1.0"));
    }

    #[test]
    fn test_version_matches_pin_tilde() {
        // ~1.2 means >=1.2.0, <1.3.0
        assert!(version_matches_pin("1.2.0", "~1.2"));
        assert!(version_matches_pin("1.2.9", "~1.2"));
        assert!(!version_matches_pin("1.3.0", "~1.2"));
    }

    #[test]
    fn test_version_matches_pin_gt() {
        assert!(version_matches_pin("2.0.0", ">=2.0.0"));
        assert!(version_matches_pin("3.0.0", ">=2.0.0"));
        assert!(!version_matches_pin("1.9.9", ">=2.0.0"));
    }

    #[test]
    fn test_version_matches_pin_with_v_prefix() {
        assert!(version_matches_pin("v1.5.0", "1.*"));
        assert!(version_matches_pin("v1.5.0", "v1.*"));
        assert!(version_matches_pin("v2.0.0", ">=2.0.0"));
    }

    #[test]
    fn test_prerelease_and_pin_roundtrip() {
        let (_dir, path) = temp_config();
        let mut config = Config::load_from(path.clone()).unwrap();

        config.apps.insert(
            "pinned-app".to_string(),
            TrackedApp {
                repo: "owner/repo".to_string(),
                asset_pattern: "*.rpm".to_string(),
                package_manager: PackageManagerType::Zypper,
                installed_version: Some("1.0.0".to_string()),
                last_checked: None,
                allow_prerelease: true,
                version_pin: Some("1.*".to_string()),
            },
        );
        config.save().unwrap();

        let loaded = Config::load_from(path).unwrap();
        let app = loaded.apps.get("pinned-app").unwrap();
        assert!(app.allow_prerelease);
        assert_eq!(app.version_pin.as_deref(), Some("1.*"));
    }

    #[test]
    fn test_prerelease_defaults_to_false() {
        let toml_str = r#"
[apps.myapp]
repo = "owner/repo"
asset_pattern = "*.rpm"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let app = config.apps.get("myapp").unwrap();
        assert!(!app.allow_prerelease);
        assert!(app.version_pin.is_none());
    }

    #[test]
    fn test_prerelease_from_toml() {
        let toml_str = r#"
[apps.beta-app]
repo = "owner/repo"
asset_pattern = "*.rpm"
allow_prerelease = true
version_pin = ">=2.0.0-beta"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let app = config.apps.get("beta-app").unwrap();
        assert!(app.allow_prerelease);
        assert_eq!(app.version_pin.as_deref(), Some(">=2.0.0-beta"));
    }
}
