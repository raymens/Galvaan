mod cli;
mod config;
mod github;
mod logging;
mod package_manager;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use tempfile::TempDir;
use tracing::info;

use cli::{Cli, Commands, ConfigAction};
use config::{AutoApprove, Config, PackageManagerType, TrackedApp};
use github::{matches_pattern, GitHubClient};
use package_manager::InstallOptions;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;

    // Initialize logging (keep guard alive for the duration of main)
    let _log_guard = logging::init(&config)?;

    info!("galvaan started");

    // Build install options: CLI flags override config
    let install_opts = InstallOptions {
        auto_approve: match &cli.auto_approve {
            Some(s) => parse_auto_approve(s)?,
            None => config.settings.auto_approve.clone(),
        },
        quiet: cli.quiet || config.settings.quiet_package_manager,
    };

    match cli.command {
        Commands::Add {
            repo,
            name,
            asset_pattern,
            package_manager,
        } => cmd_add(repo, name, asset_pattern, package_manager)?,
        Commands::Remove { name } => cmd_remove(name)?,
        Commands::List => cmd_list()?,
        Commands::Check { name } => cmd_check(name).await?,
        Commands::Update { name } => cmd_update(name, &install_opts).await?,
        Commands::Config { action } => cmd_config(action)?,
        Commands::Completions { shell } => cli::generate_completions(shell),
    }

    info!("galvaan finished");
    Ok(())
}

pub fn parse_auto_approve(s: &str) -> Result<AutoApprove> {
    match s.to_lowercase().replace('-', "_").as_str() {
        "always" => Ok(AutoApprove::Always),
        "no_deps" | "nodeps" => Ok(AutoApprove::NoDeps),
        "never" => Ok(AutoApprove::Never),
        other => anyhow::bail!(
            "Invalid auto-approve value: '{other}'. Use: always, no-deps, never"
        ),
    }
}

fn cmd_add(
    repo: String,
    name: Option<String>,
    asset_pattern: String,
    pm: Option<String>,
) -> Result<()> {
    let mut config = Config::load()?;

    let app_name = name.unwrap_or_else(|| {
        repo.split('/')
            .last()
            .unwrap_or(&repo)
            .to_string()
    });

    let pm_type = match pm {
        Some(s) => s.parse::<PackageManagerType>()?,
        None => config.settings.default_package_manager.clone(),
    };

    let app = TrackedApp {
        repo: repo.clone(),
        asset_pattern,
        package_manager: pm_type,
        installed_version: None,
        last_checked: None,
    };

    config.apps.insert(app_name.clone(), app);
    config.save()?;

    info!(app = %app_name, repo = %repo, "Added tracked app");
    println!("✓ Added '{app_name}' (tracking {repo})");
    Ok(())
}

fn cmd_remove(name: String) -> Result<()> {
    let mut config = Config::load()?;

    if config.apps.remove(&name).is_some() {
        config.save()?;
        info!(app = %name, "Removed tracked app");
        println!("✓ Removed '{name}'");
    } else {
        println!("App '{name}' not found in config");
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let config = Config::load()?;

    if config.apps.is_empty() {
        println!("No apps tracked. Use 'galvaan add' to add one.");
        return Ok(());
    }

    println!("{:<20} {:<30} {:<15} {:<10}", "NAME", "REPO", "VERSION", "PKG MGR");
    println!("{}", "-".repeat(75));

    for (name, app) in &config.apps {
        let version = app.installed_version.as_deref().unwrap_or("unknown");
        println!(
            "{:<20} {:<30} {:<15} {:<10}",
            name, app.repo, version, app.package_manager
        );
    }
    Ok(())
}

async fn cmd_check(name: Option<String>) -> Result<()> {
    let mut config = Config::load()?;
    let client = GitHubClient::new()?;

    let apps: Vec<(String, TrackedApp)> = match name {
        Some(ref n) => {
            let app = config
                .apps
                .get(n)
                .with_context(|| format!("App '{n}' not found"))?
                .clone();
            vec![(n.clone(), app)]
        }
        None => config.apps.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    };

    for (app_name, app) in &apps {
        print!("Checking {app_name}... ");
        match client.get_latest_release(&app.repo).await {
            Ok(release) => {
                let current = app.installed_version.as_deref().unwrap_or("none");
                let latest = &release.tag_name;
                if current == *latest || current == latest.trim_start_matches('v') {
                    println!("✓ up to date ({latest})");
                } else {
                    println!("⬆ update available: {current} → {latest}");
                }
                info!(app = %app_name, current = %current, latest = %latest, "Checked for updates");
            }
            Err(e) => {
                println!("✗ error: {e}");
                info!(app = %app_name, error = %e, "Check failed");
            }
        }

        if let Some(tracked) = config.apps.get_mut(app_name) {
            tracked.last_checked = Some(Utc::now().to_rfc3339());
        }
    }

    config.save()?;
    Ok(())
}

async fn cmd_update(name: Option<String>, install_opts: &InstallOptions) -> Result<()> {
    let mut config = Config::load()?;
    let client = GitHubClient::new()?;

    let apps: Vec<(String, TrackedApp)> = match name {
        Some(ref n) => {
            let app = config
                .apps
                .get(n)
                .with_context(|| format!("App '{n}' not found"))?
                .clone();
            vec![(n.clone(), app)]
        }
        None => config.apps.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    };

    for (app_name, app) in &apps {
        println!("Checking {app_name} for updates...");
        let release = match client.get_latest_release(&app.repo).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  ✗ Failed to check {app_name}: {e}");
                continue;
            }
        };

        let latest = release.tag_name.trim_start_matches('v').to_string();
        let current = app.installed_version.as_deref().unwrap_or("");

        if current == latest || current == release.tag_name {
            println!("  ✓ {app_name} is already up to date ({latest})");
            continue;
        }

        // Find matching asset
        let asset = release
            .assets
            .iter()
            .find(|a| matches_pattern(&a.name, &app.asset_pattern));

        let asset = match asset {
            Some(a) => a,
            None => {
                eprintln!(
                    "  ✗ No asset matching '{}' found in release {}",
                    app.asset_pattern, release.tag_name
                );
                eprintln!("    Available assets:");
                for a in &release.assets {
                    eprintln!("      - {}", a.name);
                }
                continue;
            }
        };

        println!(
            "  Downloading {} ({:.1} MB)...",
            asset.name,
            asset.size as f64 / 1_048_576.0
        );
        info!(app = %app_name, asset = %asset.name, size = asset.size, "Downloading asset");

        let tmp_dir = TempDir::new().context("Failed to create temp directory")?;
        let download_path = tmp_dir.path().join(&asset.name);

        client
            .download_asset(&asset.browser_download_url, &download_path, asset.size)
            .await?;

        // Install via package manager
        let pm = package_manager::create(&app.package_manager);
        pm.install(&download_path, install_opts)?;

        // Update config with new version
        if let Some(tracked) = config.apps.get_mut(app_name) {
            tracked.installed_version = Some(latest.clone());
            tracked.last_checked = Some(Utc::now().to_rfc3339());
        }

        info!(app = %app_name, version = %latest, "Updated successfully");
        println!("  ✓ {app_name} updated to {latest}");
    }

    config.save()?;
    Ok(())
}

fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let config = Config::load()?;
            println!("auto_approve = {}", config.settings.auto_approve);
            println!("default_package_manager = {}", config.settings.default_package_manager);
            println!("quiet_package_manager = {}", config.settings.quiet_package_manager);
            println!(
                "log_file = {}",
                config.settings.log_file.as_deref().unwrap_or("(not set)")
            );
            println!("log_level = {}", config.settings.log_level);
        }
        ConfigAction::Set { key, value } => {
            let mut config = Config::load()?;
            match key.as_str() {
                "auto_approve" => {
                    config.settings.auto_approve = parse_auto_approve(&value)?;
                }
                "default_package_manager" => {
                    config.settings.default_package_manager = value.parse::<PackageManagerType>()?;
                }
                "quiet_package_manager" => {
                    config.settings.quiet_package_manager = value
                        .parse::<bool>()
                        .context("Value must be 'true' or 'false'")?;
                }
                "log_file" => {
                    if value == "none" || value.is_empty() {
                        config.settings.log_file = None;
                    } else {
                        config.settings.log_file = Some(value.clone());
                    }
                }
                "log_level" => {
                    let valid = ["trace", "debug", "info", "warn", "error"];
                    if !valid.contains(&value.to_lowercase().as_str()) {
                        anyhow::bail!("Invalid log level: '{value}'. Use: {}", valid.join(", "));
                    }
                    config.settings.log_level = value.to_lowercase();
                }
                other => anyhow::bail!(
                    "Unknown setting: '{other}'. Available: auto_approve, default_package_manager, quiet_package_manager, log_file, log_level"
                ),
            }
            config.save()?;
            println!("✓ Set {key} = {value}");
        }
        ConfigAction::Path => {
            println!("{}", Config::default_config_path()?.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_parse_auto_approve_valid() {
        assert_eq!(parse_auto_approve("always").unwrap(), AutoApprove::Always);
        assert_eq!(parse_auto_approve("ALWAYS").unwrap(), AutoApprove::Always);
        assert_eq!(parse_auto_approve("no_deps").unwrap(), AutoApprove::NoDeps);
        assert_eq!(parse_auto_approve("no-deps").unwrap(), AutoApprove::NoDeps);
        assert_eq!(parse_auto_approve("nodeps").unwrap(), AutoApprove::NoDeps);
        assert_eq!(parse_auto_approve("never").unwrap(), AutoApprove::Never);
        assert_eq!(parse_auto_approve("Never").unwrap(), AutoApprove::Never);
    }

    #[test]
    fn test_parse_auto_approve_invalid() {
        assert!(parse_auto_approve("yes").is_err());
        assert!(parse_auto_approve("").is_err());
        assert!(parse_auto_approve("auto").is_err());
    }

    #[test]
    fn test_cli_parse_list() {
        let cli = Cli::parse_from(["galvaan", "list"]);
        assert!(matches!(cli.command, Commands::List));
        assert!(!cli.quiet);
        assert!(cli.auto_approve.is_none());
    }

    #[test]
    fn test_cli_parse_add() {
        let cli = Cli::parse_from([
            "galvaan", "add", "github/app",
            "--name", "copilot",
            "--asset-pattern", "*-linux-x64.rpm",
        ]);
        match cli.command {
            Commands::Add { repo, name, asset_pattern, package_manager } => {
                assert_eq!(repo, "github/app");
                assert_eq!(name.as_deref(), Some("copilot"));
                assert_eq!(asset_pattern, "*-linux-x64.rpm");
                assert!(package_manager.is_none()); // defaults to config setting
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_add_with_explicit_pm() {
        let cli = Cli::parse_from([
            "galvaan", "add", "github/app",
            "--asset-pattern", "*.rpm",
            "--package-manager", "zypper",
        ]);
        match cli.command {
            Commands::Add { package_manager, .. } => {
                assert_eq!(package_manager.as_deref(), Some("zypper"));
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_update_with_name() {
        let cli = Cli::parse_from(["galvaan", "update", "copilot"]);
        match cli.command {
            Commands::Update { name } => assert_eq!(name.as_deref(), Some("copilot")),
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parse_update_all() {
        let cli = Cli::parse_from(["galvaan", "update"]);
        match cli.command {
            Commands::Update { name } => assert!(name.is_none()),
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parse_quiet_flag() {
        let cli = Cli::parse_from(["galvaan", "-q", "update"]);
        assert!(cli.quiet);
    }

    #[test]
    fn test_cli_parse_auto_approve_flag() {
        let cli = Cli::parse_from(["galvaan", "--auto-approve", "always", "update"]);
        assert_eq!(cli.auto_approve.as_deref(), Some("always"));
    }

    #[test]
    fn test_cli_parse_config_show() {
        let cli = Cli::parse_from(["galvaan", "config", "show"]);
        match cli.command {
            Commands::Config { action: ConfigAction::Show } => {}
            _ => panic!("Expected Config Show"),
        }
    }

    #[test]
    fn test_cli_parse_config_set() {
        let cli = Cli::parse_from(["galvaan", "config", "set", "auto_approve", "always"]);
        match cli.command {
            Commands::Config { action: ConfigAction::Set { key, value } } => {
                assert_eq!(key, "auto_approve");
                assert_eq!(value, "always");
            }
            _ => panic!("Expected Config Set"),
        }
    }

    #[test]
    fn test_cli_parse_config_path() {
        let cli = Cli::parse_from(["galvaan", "config", "path"]);
        match cli.command {
            Commands::Config { action: ConfigAction::Path } => {}
            _ => panic!("Expected Config Path"),
        }
    }

    #[test]
    fn test_cli_parse_remove() {
        let cli = Cli::parse_from(["galvaan", "remove", "myapp"]);
        match cli.command {
            Commands::Remove { name } => assert_eq!(name, "myapp"),
            _ => panic!("Expected Remove command"),
        }
    }

    #[test]
    fn test_cli_parse_check_specific() {
        let cli = Cli::parse_from(["galvaan", "check", "copilot"]);
        match cli.command {
            Commands::Check { name } => assert_eq!(name.as_deref(), Some("copilot")),
            _ => panic!("Expected Check command"),
        }
    }

    #[test]
    fn test_cli_parse_check_all() {
        let cli = Cli::parse_from(["galvaan", "check"]);
        match cli.command {
            Commands::Check { name } => assert!(name.is_none()),
            _ => panic!("Expected Check command"),
        }
    }

    #[test]
    fn test_install_options_from_config_defaults() {
        let config = Config::default();
        let opts = InstallOptions {
            auto_approve: config.settings.auto_approve.clone(),
            quiet: config.settings.quiet_package_manager,
        };
        assert_eq!(opts.auto_approve, AutoApprove::NoDeps);
        assert!(!opts.quiet);
    }

    #[test]
    fn test_install_options_cli_overrides() {
        let config = Config::default();
        // Simulate CLI --auto-approve always -q
        let auto_approve = parse_auto_approve("always").unwrap();
        let opts = InstallOptions {
            auto_approve,
            quiet: true || config.settings.quiet_package_manager,
        };
        assert_eq!(opts.auto_approve, AutoApprove::Always);
        assert!(opts.quiet);
    }

    #[test]
    fn test_app_name_defaults_to_repo_name() {
        let repo = "github/app";
        let app_name = repo.split('/').last().unwrap_or(repo).to_string();
        assert_eq!(app_name, "app");
    }

    #[test]
    fn test_app_name_handles_no_slash() {
        let repo = "singlename";
        let app_name = repo.split('/').last().unwrap_or(repo).to_string();
        assert_eq!(app_name, "singlename");
    }

    #[test]
    fn test_cli_parse_completions() {
        let cli = Cli::parse_from(["galvaan", "completions", "bash"]);
        match cli.command {
            Commands::Completions { shell } => {
                assert_eq!(shell, clap_complete::Shell::Bash);
            }
            _ => panic!("Expected Completions command"),
        }
    }

    #[test]
    fn test_cli_parse_completions_zsh() {
        let cli = Cli::parse_from(["galvaan", "completions", "zsh"]);
        match cli.command {
            Commands::Completions { shell } => {
                assert_eq!(shell, clap_complete::Shell::Zsh);
            }
            _ => panic!("Expected Completions command"),
        }
    }

    #[test]
    fn test_cli_parse_completions_fish() {
        let cli = Cli::parse_from(["galvaan", "completions", "fish"]);
        match cli.command {
            Commands::Completions { shell } => {
                assert_eq!(shell, clap_complete::Shell::Fish);
            }
            _ => panic!("Expected Completions command"),
        }
    }

    #[test]
    fn test_package_manager_from_str() {
        assert_eq!(
            "zypper".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Zypper
        );
        assert_eq!(
            "Zypper".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Zypper
        );
        assert_eq!(
            "ZYPPER".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Zypper
        );
        assert_eq!(
            "dnf".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Dnf
        );
        assert_eq!(
            "apt".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Apt
        );
        assert_eq!(
            "apt-get".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Apt
        );
        assert_eq!(
            "pacman".parse::<PackageManagerType>().unwrap(),
            PackageManagerType::Pacman
        );
        assert!("flatpak".parse::<PackageManagerType>().is_err());
        assert!("".parse::<PackageManagerType>().is_err());
    }

    #[test]
    fn test_default_package_manager_from_config() {
        let config = Config::default();
        assert_eq!(config.settings.default_package_manager, PackageManagerType::Zypper);
    }
}
