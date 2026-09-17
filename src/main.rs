mod cli;
mod config;
mod github;
mod logging;
mod package_manager;
mod url_source;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use tempfile::TempDir;
use tracing::info;

use cli::{Cli, Commands, ConfigAction};
use config::{AutoApprove, Config, PackageManagerType, SourceKind, TrackedApp, detect_source_kind};
use github::{GitHubClient, matches_pattern};
use package_manager::InstallOptions;
use url_source::{UrlSourceClient, best_effort_version_from_filename, should_update_for_identity};

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
        allow_unsigned: false,   // overridden per-app in cmd_update
        ignore_checksums: false, // overridden per-app in cmd_update
    };

    match cli.command {
        Commands::Add {
            source,
            name,
            asset_pattern,
            package_manager,
            prerelease,
            pin,
            allow_unsigned,
            ignore_checksums,
        } => cmd_add(
            source,
            name,
            asset_pattern,
            package_manager,
            prerelease,
            pin,
            allow_unsigned,
            ignore_checksums,
        )?,
        Commands::Remove { name } => cmd_remove(name)?,
        Commands::List => cmd_list()?,
        Commands::Check { name, prerelease } => cmd_check(name, prerelease).await?,
        Commands::Update {
            name,
            version,
            prerelease,
        } => cmd_update(name, &install_opts, version, prerelease).await?,
        Commands::Pin { name, constraint } => cmd_pin(name, constraint)?,
        Commands::Unpin { name } => cmd_unpin(name)?,
        Commands::IgnoreChecksums { name } => cmd_ignore_checksums(name)?,
        Commands::VerifyChecksums { name } => cmd_verify_checksums(name)?,
        Commands::AllowUnsigned { name } => cmd_allow_unsigned(name)?,
        Commands::VerifyUnsigned { name } => cmd_verify_unsigned(name)?,
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
        other => {
            anyhow::bail!("Invalid auto-approve value: '{other}'. Use: always, no-deps, never")
        }
    }
}

/// Auto-detect an asset pattern based on the package manager and system architecture.
fn detect_asset_pattern(pm: &PackageManagerType) -> String {
    let arch = std::env::consts::ARCH;
    let arch_patterns: &[&str] = match arch {
        "x86_64" => &["x86_64", "x64", "amd64"],
        "aarch64" => &["aarch64", "arm64"],
        other => &[other],
    };

    let extension = match pm {
        PackageManagerType::Zypper | PackageManagerType::Dnf => ".rpm",
        PackageManagerType::Apt => ".deb",
        PackageManagerType::Pacman => ".pkg.tar.zst",
    };

    // Build a pattern like "*{linux}*{arch}*{ext}" — use the first arch variant
    let arch_str = arch_patterns[0];
    format!("*linux*{arch_str}*{extension}")
}

fn source_type_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Github => "github",
        SourceKind::Url => "url",
    }
}

fn default_app_name_from_source(source: &str) -> String {
    match detect_source_kind(source) {
        Ok(SourceKind::Github) => source.split('/').next_back().unwrap_or(source).to_string(),
        Ok(SourceKind::Url) => {
            let tail = source
                .split('/')
                .next_back()
                .unwrap_or(source)
                .split('?')
                .next()
                .unwrap_or(source)
                .split('#')
                .next()
                .unwrap_or(source)
                .trim();
            if tail.is_empty() {
                "url-package".to_string()
            } else {
                tail.to_string()
            }
        }
        Err(_) => source.to_string(),
    }
}

fn cmd_add(
    source: String,
    name: Option<String>,
    asset_pattern: Option<String>,
    pm: Option<String>,
    allow_prerelease: bool,
    version_pin: Option<String>,
    allow_unsigned: bool,
    ignore_checksums: bool,
) -> Result<()> {
    let mut config = Config::load()?;

    let source_kind = detect_source_kind(&source)?;

    if source_kind == SourceKind::Url {
        if allow_prerelease {
            anyhow::bail!("--prerelease is only supported for GitHub sources (owner/repo)");
        }
        if version_pin.is_some() {
            anyhow::bail!("--pin is only supported for GitHub sources (owner/repo)");
        }
    }

    let app_name = name.unwrap_or_else(|| default_app_name_from_source(&source));

    let pm_type = match pm {
        Some(s) => s.parse::<PackageManagerType>()?,
        None => config.settings.default_package_manager.clone(),
    };

    let pattern = match asset_pattern {
        Some(p) => p,
        None => detect_asset_pattern(&pm_type),
    };

    if source_kind == SourceKind::Github
        && let Some(ref pin) = version_pin
    {
        validate_version_pin(pin)?;
    }

    let app = TrackedApp {
        source: source.clone(),
        asset_pattern: pattern.clone(),
        package_manager: pm_type,
        installed_version: None,
        last_checked: None,
        allow_prerelease,
        version_pin: version_pin.clone(),
        allow_unsigned,
        ignore_checksums,
        url_identity: None,
    };

    config.apps.insert(app_name.clone(), app);
    config.save()?;

    info!(
        app = %app_name,
        source = %source,
        source_kind = source_type_label(source_kind),
        "Added tracked app"
    );
    let mut msg = format!(
        "✓ Added '{app_name}' ([{}] {source})",
        source_type_label(source_kind)
    );
    if source_kind == SourceKind::Github {
        msg.push_str(&format!(" [pattern: {pattern}]"));
    }
    if allow_prerelease {
        msg.push_str(" [prereleases enabled]");
    }
    if let Some(pin) = &version_pin {
        msg.push_str(&format!(" [pinned: {pin}]"));
    }
    if allow_unsigned {
        msg.push_str(" [allow unsigned packages]");
    }
    if ignore_checksums {
        msg.push_str(" [signature verification disabled]");
    }
    println!("{msg}");
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

fn cmd_pin(name: String, constraint: String) -> Result<()> {
    let mut config = Config::load()?;
    validate_version_pin(&constraint)?;

    let app = config
        .apps
        .get_mut(&name)
        .with_context(|| format!("App '{name}' not found"))?;

    if app.source_kind()? == SourceKind::Url {
        anyhow::bail!("pin is only supported for GitHub sources (owner/repo)");
    }

    app.version_pin = Some(constraint.clone());
    config.save()?;
    println!("✓ Pinned '{name}' to {constraint}");
    Ok(())
}

fn cmd_unpin(name: String) -> Result<()> {
    let mut config = Config::load()?;

    let app = config
        .apps
        .get_mut(&name)
        .with_context(|| format!("App '{name}' not found"))?;

    if app.version_pin.is_some() {
        app.version_pin = None;
        config.save()?;
        println!("✓ Removed version pin from '{name}'");
    } else {
        println!("'{name}' is not pinned");
    }
    Ok(())
}

fn cmd_ignore_checksums(name: String) -> Result<()> {
    let mut config = Config::load()?;

    let app = config
        .apps
        .get_mut(&name)
        .with_context(|| format!("App '{name}' not found"))?;

    if app.ignore_checksums {
        println!("'{name}' already has checksum verification disabled");
    } else {
        app.ignore_checksums = true;
        config.save()?;
        info!(app = %name, "Disabled checksum verification");
        println!("✓ Disabled checksum/signature verification for '{name}'");
    }
    Ok(())
}

fn cmd_verify_checksums(name: String) -> Result<()> {
    let mut config = Config::load()?;

    let app = config
        .apps
        .get_mut(&name)
        .with_context(|| format!("App '{name}' not found"))?;

    if !app.ignore_checksums {
        println!("'{name}' already has checksum verification enabled");
    } else {
        app.ignore_checksums = false;
        config.save()?;
        info!(app = %name, "Enabled checksum verification");
        println!("✓ Re-enabled checksum/signature verification for '{name}'");
    }
    Ok(())
}

fn cmd_allow_unsigned(name: String) -> Result<()> {
    let mut config = Config::load()?;

    let app = config
        .apps
        .get_mut(&name)
        .with_context(|| format!("App '{name}' not found"))?;

    if app.allow_unsigned {
        println!("'{name}' already allows unsigned packages");
    } else {
        app.allow_unsigned = true;
        config.save()?;
        info!(app = %name, "Allowed unsigned packages");
        println!("✓ Allowed unsigned packages for '{name}'");
    }
    Ok(())
}

fn cmd_verify_unsigned(name: String) -> Result<()> {
    let mut config = Config::load()?;

    let app = config
        .apps
        .get_mut(&name)
        .with_context(|| format!("App '{name}' not found"))?;

    if !app.allow_unsigned {
        println!("'{name}' already verifies unsigned-package status");
    } else {
        app.allow_unsigned = false;
        config.save()?;
        info!(app = %name, "Re-enabled unsigned-package checks");
        println!("✓ Re-enabled unsigned-package checks for '{name}'");
    }
    Ok(())
}

/// Validate that a version pin string is parseable
fn validate_version_pin(pin: &str) -> Result<()> {
    use config::version_matches_pin;
    // Try matching against a dummy version to ensure the pin is syntactically valid
    // Wildcard and exact pins always work; semver ranges may fail to parse
    if (pin.starts_with('>')
        || pin.starts_with('<')
        || pin.starts_with('^')
        || pin.starts_with('~')
        || pin.contains(','))
        && semver::VersionReq::parse(pin.trim_start_matches('v')).is_err()
    {
        anyhow::bail!(
            "Invalid version constraint: '{pin}'. Examples: '1.0.24', '1.*', '>=2.0.0,<3.0.0', '^1.0'"
        );
    }
    // Quick sanity check — a pin like "" is invalid
    if pin.is_empty() {
        anyhow::bail!("Version pin cannot be empty");
    }
    // Run through the match function to make sure it doesn't panic
    let _ = version_matches_pin("0.0.0", pin);
    Ok(())
}

fn cmd_list() -> Result<()> {
    let config = Config::load()?;

    if config.apps.is_empty() {
        println!("No apps tracked. Use 'galvaan add' to add one.");
        return Ok(());
    }

    println!(
        "{:<20} {:<9} {:<30} {:<15} {:<10} STATE",
        "NAME", "SOURCE", "TARGET", "VERSION", "PKG MGR"
    );
    println!("{}", "-".repeat(110));

    for (name, app) in &config.apps {
        let version = app.installed_version.as_deref().unwrap_or("unknown");
        let source_kind = app
            .source_kind()
            .map(source_type_label)
            .unwrap_or("invalid");
        let mut flags = Vec::new();
        if app.allow_prerelease {
            flags.push("prerelease".to_string());
        }
        if let Some(ref pin) = app.version_pin {
            flags.push(format!("pin:{pin}"));
        }
        if app.allow_unsigned {
            flags.push("allow-unsigned".to_string());
        }
        if app.ignore_checksums {
            flags.push("ignore-checksums".to_string());
        }
        if source_kind == "url" {
            let identity_state = if app.url_identity.is_some() {
                "identity:known"
            } else {
                "identity:missing"
            };
            flags.push(identity_state.to_string());
        }
        let flags_str = if flags.is_empty() {
            String::new()
        } else {
            flags.join(", ")
        };
        println!(
            "{:<20} [{:<7}] {:<30} {:<15} {:<10} {}",
            name, source_kind, app.source, version, app.package_manager, flags_str
        );
    }
    Ok(())
}

async fn cmd_check(name: Option<String>, prerelease_override: bool) -> Result<()> {
    let mut config = Config::load()?;
    let client = GitHubClient::new()?;
    let url_client = UrlSourceClient::new()?;
    let mut had_errors = false;

    let apps: Vec<(String, TrackedApp)> = match name {
        Some(ref n) => {
            let app = config
                .apps
                .get(n)
                .with_context(|| format!("App '{n}' not found"))?
                .clone();
            vec![(n.clone(), app)]
        }
        None => config
            .apps
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };

    for (app_name, app) in &apps {
        let source_kind = match app.source_kind() {
            Ok(kind) => kind,
            Err(e) => {
                println!("Checking {app_name} [invalid]... ✗ error: {e}");
                had_errors = true;
                if let Some(tracked) = config.apps.get_mut(app_name) {
                    tracked.last_checked = Some(Utc::now().to_rfc3339());
                }
                continue;
            }
        };

        print!(
            "Checking {app_name} [{}]... ",
            source_type_label(source_kind)
        );

        match source_kind {
            SourceKind::Github => {
                let allow_pre = prerelease_override || app.allow_prerelease;
                let needs_release_list = allow_pre || app.version_pin.is_some();

                let release_result = if needs_release_list {
                    match client.get_releases(&app.source, 50).await {
                        Ok(releases) => {
                            let filter = github::ReleaseFilter {
                                allow_prerelease: allow_pre,
                                version_pin: app.version_pin.as_deref(),
                                specific_version: None,
                            };
                            match github::find_best_release(&releases, &filter) {
                                Some(r) => Ok(r.clone()),
                                None => {
                                    let mut reason = String::from("no matching release found");
                                    if let Some(ref pin) = app.version_pin {
                                        reason.push_str(&format!(" for pin '{pin}'"));
                                    }
                                    Err(anyhow::anyhow!(reason))
                                }
                            }
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    client.get_latest_release(&app.source).await
                };

                match release_result {
                    Ok(release) => {
                        let current = app.installed_version.as_deref().unwrap_or("none");
                        let latest = &release.tag_name;
                        let pre_label = if release.prerelease {
                            " (prerelease)"
                        } else {
                            ""
                        };
                        if current == *latest || current == latest.trim_start_matches('v') {
                            println!("✓ up to date ({latest}){pre_label}");
                        } else {
                            println!("⬆ update available: {current} -> {latest}{pre_label}");
                        }
                        info!(app = %app_name, current = %current, latest = %latest, "Checked for updates");
                    }
                    Err(e) => {
                        println!("✗ error: {e}");
                        had_errors = true;
                        info!(app = %app_name, error = %e, "Check failed");
                    }
                }
            }
            SourceKind::Url => {
                if prerelease_override {
                    println!("✗ error: --prerelease is only supported for GitHub sources");
                    had_errors = true;
                } else if app.version_pin.is_some() {
                    println!("✗ error: version pinning is only supported for GitHub sources");
                    had_errors = true;
                } else {
                    match url_client.probe_identity(&app.source).await {
                        Ok(probe) => {
                            let changed = should_update_for_identity(
                                app.url_identity.as_deref(),
                                &probe.identity,
                            );
                            let installed = app.installed_version.as_deref().unwrap_or("unknown");
                            if changed {
                                println!(
                                    "⬆ update available: identity changed (installed: {installed})"
                                );
                            } else {
                                println!(
                                    "✓ up to date (identity unchanged, installed: {installed})"
                                );
                            }
                        }
                        Err(e) => {
                            println!("✗ error: {e}");
                            had_errors = true;
                        }
                    }
                }
            }
        }

        if let Some(tracked) = config.apps.get_mut(app_name) {
            tracked.last_checked = Some(Utc::now().to_rfc3339());
        }
    }

    config.save()?;
    if had_errors {
        anyhow::bail!("One or more checks failed");
    }
    Ok(())
}

async fn cmd_update(
    name: Option<String>,
    install_opts: &InstallOptions,
    specific_version: Option<String>,
    prerelease_override: bool,
) -> Result<()> {
    let mut config = Config::load()?;
    let client = GitHubClient::new()?;
    let url_client = UrlSourceClient::new()?;
    let mut had_errors = false;

    let apps: Vec<(String, TrackedApp)> = match name {
        Some(ref n) => {
            let app = config
                .apps
                .get(n)
                .with_context(|| format!("App '{n}' not found"))?
                .clone();
            vec![(n.clone(), app)]
        }
        None => config
            .apps
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };

    // --version only makes sense for a single app
    if specific_version.is_some() && apps.len() > 1 {
        anyhow::bail!("--version can only be used when updating a single app");
    }

    for (app_name, app) in &apps {
        let source_kind = match app.source_kind() {
            Ok(kind) => kind,
            Err(e) => {
                eprintln!("Checking {app_name} [invalid]... ✗ {e}");
                had_errors = true;
                continue;
            }
        };

        println!(
            "Checking {app_name} [{}] for updates...",
            source_type_label(source_kind)
        );

        match source_kind {
            SourceKind::Github => {
                let allow_pre = prerelease_override || app.allow_prerelease;

                let release = if let Some(ref ver) = specific_version {
                    let tag_attempts = if ver.starts_with('v') {
                        vec![ver.clone(), ver.trim_start_matches('v').to_string()]
                    } else {
                        vec![format!("v{ver}"), ver.clone()]
                    };
                    let mut found = None;
                    for tag in &tag_attempts {
                        match client.get_release_by_tag(&app.source, tag).await {
                            Ok(r) => {
                                found = Some(r);
                                break;
                            }
                            Err(_) => continue,
                        }
                    }
                    match found {
                        Some(r) => r,
                        None => {
                            eprintln!("  ✗ Version '{ver}' not found for {}", app.source);
                            had_errors = true;
                            continue;
                        }
                    }
                } else {
                    let needs_release_list = allow_pre || app.version_pin.is_some();
                    if needs_release_list {
                        match client.get_releases(&app.source, 50).await {
                            Ok(releases) => {
                                let filter = github::ReleaseFilter {
                                    allow_prerelease: allow_pre,
                                    version_pin: app.version_pin.as_deref(),
                                    specific_version: None,
                                };
                                match github::find_best_release(&releases, &filter) {
                                    Some(r) => r.clone(),
                                    None => {
                                        let mut reason =
                                            format!("  ✗ No matching release found for {app_name}");
                                        if let Some(ref pin) = app.version_pin {
                                            reason.push_str(&format!(" (pin: {pin})"));
                                        }
                                        eprintln!("{reason}");
                                        had_errors = true;
                                        continue;
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("  ✗ Failed to check {app_name}: {e}");
                                had_errors = true;
                                continue;
                            }
                        }
                    } else {
                        match client.get_latest_release(&app.source).await {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("  ✗ Failed to check {app_name}: {e}");
                                had_errors = true;
                                continue;
                            }
                        }
                    }
                };

                let latest = release.tag_name.trim_start_matches('v').to_string();
                let current = app.installed_version.as_deref().unwrap_or("");

                if specific_version.is_none() && (current == latest || current == release.tag_name)
                {
                    let pre_label = if release.prerelease {
                        " (prerelease)"
                    } else {
                        ""
                    };
                    println!("  ✓ {app_name} is already up to date ({latest}){pre_label}");
                    continue;
                }

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
                        had_errors = true;
                        continue;
                    }
                };

                let pre_label = if release.prerelease {
                    " (prerelease)"
                } else {
                    ""
                };
                println!(
                    "  Downloading {} ({:.1} MB)...{pre_label}",
                    asset.name,
                    asset.size as f64 / 1_048_576.0
                );
                info!(app = %app_name, asset = %asset.name, size = asset.size, version = %latest, "Downloading asset");

                let tmp_dir = match TempDir::new().context("Failed to create temp directory") {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("  ✗ {e}");
                        had_errors = true;
                        continue;
                    }
                };
                let download_path = tmp_dir.path().join(&asset.name);

                if let Err(e) = client
                    .download_asset(&asset.browser_download_url, &download_path, asset.size)
                    .await
                {
                    eprintln!("  ✗ Download failed for {app_name}: {e}");
                    had_errors = true;
                    continue;
                }

                let pm = package_manager::create(&app.package_manager);
                let mut app_install_opts = install_opts.clone();
                app_install_opts.allow_unsigned = app.allow_unsigned;
                app_install_opts.ignore_checksums = app.ignore_checksums;
                if let Err(e) = pm.install(&download_path, &app_install_opts) {
                    eprintln!("  ✗ Install failed for {app_name}: {e}");
                    had_errors = true;
                    continue;
                }

                if let Some(tracked) = config.apps.get_mut(app_name) {
                    tracked.installed_version = Some(latest.clone());
                    tracked.last_checked = Some(Utc::now().to_rfc3339());
                }
                if let Err(e) = config.save() {
                    eprintln!("  ✗ Failed to persist state for {app_name}: {e}");
                    had_errors = true;
                    continue;
                }

                info!(app = %app_name, version = %latest, "Updated successfully");
                println!("  ✓ {app_name} updated to {latest}{pre_label}");
            }
            SourceKind::Url => {
                if specific_version.is_some() {
                    eprintln!("  ✗ --version is only supported for GitHub sources");
                    had_errors = true;
                    continue;
                }
                if prerelease_override {
                    eprintln!("  ✗ --prerelease is only supported for GitHub sources");
                    had_errors = true;
                    continue;
                }
                if app.version_pin.is_some() {
                    eprintln!("  ✗ version pinning is only supported for GitHub sources");
                    had_errors = true;
                    continue;
                }

                let probe = match url_client.probe_identity(&app.source).await {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("  ✗ Failed to probe URL metadata for {app_name}: {e}");
                        had_errors = true;
                        continue;
                    }
                };

                if !should_update_for_identity(app.url_identity.as_deref(), &probe.identity) {
                    println!("  ✓ {app_name} is already up to date (identity unchanged)");
                    continue;
                }

                let filename = probe
                    .filename
                    .clone()
                    .unwrap_or_else(|| format!("{app_name}.pkg"));
                if let Some(size) = probe.content_length {
                    println!(
                        "  Downloading {} ({:.1} MB)...",
                        filename,
                        size as f64 / 1_048_576.0
                    );
                } else {
                    println!("  Downloading {}...", filename);
                }

                let tmp_dir = match TempDir::new().context("Failed to create temp directory") {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("  ✗ {e}");
                        had_errors = true;
                        continue;
                    }
                };
                let download_path = tmp_dir.path().join(&filename);

                if let Err(e) = url_client
                    .download_package(&probe.resolved_url, &download_path, probe.content_length)
                    .await
                {
                    eprintln!("  ✗ Download failed for {app_name}: {e}");
                    had_errors = true;
                    continue;
                }

                let pm = package_manager::create(&app.package_manager);
                let mut app_install_opts = install_opts.clone();
                app_install_opts.allow_unsigned = app.allow_unsigned;
                app_install_opts.ignore_checksums = app.ignore_checksums;
                if let Err(e) = pm.install(&download_path, &app_install_opts) {
                    eprintln!("  ✗ Install failed for {app_name}: {e}");
                    had_errors = true;
                    continue;
                }

                let mut detected_version = pm.installed_version(app_name).ok().flatten();
                if detected_version.is_none() {
                    detected_version = best_effort_version_from_filename(&filename);
                }

                if let Some(tracked) = config.apps.get_mut(app_name) {
                    if let Some(version) = detected_version {
                        tracked.installed_version = Some(version);
                    }
                    tracked.url_identity = Some(probe.identity);
                    tracked.last_checked = Some(Utc::now().to_rfc3339());
                }
                if let Err(e) = config.save() {
                    eprintln!("  ✗ Failed to persist state for {app_name}: {e}");
                    had_errors = true;
                    continue;
                }

                println!("  ✓ {app_name} updated from URL source");
            }
        }
    }

    if had_errors {
        anyhow::bail!("One or more updates failed");
    }
    Ok(())
}

fn cmd_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let config = Config::load()?;
            println!("auto_approve = {}", config.settings.auto_approve);
            println!(
                "default_package_manager = {}",
                config.settings.default_package_manager
            );
            println!(
                "quiet_package_manager = {}",
                config.settings.quiet_package_manager
            );
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
                    config.settings.default_package_manager =
                        value.parse::<PackageManagerType>()?;
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
            "galvaan",
            "add",
            "github/app",
            "--name",
            "copilot",
            "--asset-pattern",
            "*-linux-x64.rpm",
        ]);
        match cli.command {
            Commands::Add {
                source,
                name,
                asset_pattern,
                package_manager,
                prerelease,
                pin,
                allow_unsigned,
                ignore_checksums,
            } => {
                assert_eq!(source, "github/app");
                assert_eq!(name.as_deref(), Some("copilot"));
                assert_eq!(asset_pattern.as_deref(), Some("*-linux-x64.rpm"));
                assert!(package_manager.is_none());
                assert!(!prerelease);
                assert!(pin.is_none());
                assert!(!allow_unsigned);
                assert!(!ignore_checksums);
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_add_with_prerelease_and_pin() {
        let cli = Cli::parse_from([
            "galvaan",
            "add",
            "owner/repo",
            "--asset-pattern",
            "*.rpm",
            "--prerelease",
            "--pin",
            "1.*",
        ]);
        match cli.command {
            Commands::Add {
                prerelease, pin, ..
            } => {
                assert!(prerelease);
                assert_eq!(pin.as_deref(), Some("1.*"));
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_add_with_ignore_checksums_alias() {
        let cli = Cli::parse_from([
            "galvaan",
            "add",
            "owner/repo",
            "--asset-pattern",
            "*.rpm",
            "--ignore-checksums",
        ]);
        match cli.command {
            Commands::Add {
                ignore_checksums, ..
            } => assert!(ignore_checksums),
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_add_with_allow_unsigned_rpm_alias() {
        let cli = Cli::parse_from([
            "galvaan",
            "add",
            "owner/repo",
            "--asset-pattern",
            "*.rpm",
            "--allow-unsigned-rpm",
        ]);
        match cli.command {
            Commands::Add { allow_unsigned, .. } => assert!(allow_unsigned),
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_add_with_ignore_checksums_and_allow_unsigned() {
        let cli = Cli::parse_from([
            "galvaan",
            "add",
            "owner/repo",
            "--asset-pattern",
            "*.rpm",
            "--ignore-checksums",
            "--allow-unsigned-rpm",
        ]);
        match cli.command {
            Commands::Add {
                allow_unsigned,
                ignore_checksums,
                ..
            } => {
                assert!(allow_unsigned);
                assert!(ignore_checksums);
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_ignore_checksums() {
        let cli = Cli::parse_from(["galvaan", "ignore-checksums", "copilot"]);
        match cli.command {
            Commands::IgnoreChecksums { name } => assert_eq!(name, "copilot"),
            _ => panic!("Expected IgnoreChecksums command"),
        }
    }

    #[test]
    fn test_cli_parse_verify_checksums() {
        let cli = Cli::parse_from(["galvaan", "verify-checksums", "copilot"]);
        match cli.command {
            Commands::VerifyChecksums { name } => assert_eq!(name, "copilot"),
            _ => panic!("Expected VerifyChecksums command"),
        }
    }

    #[test]
    fn test_cli_parse_allow_unsigned() {
        let cli = Cli::parse_from(["galvaan", "allow-unsigned", "copilot"]);
        match cli.command {
            Commands::AllowUnsigned { name } => assert_eq!(name, "copilot"),
            _ => panic!("Expected AllowUnsigned command"),
        }
    }

    #[test]
    fn test_cli_parse_verify_unsigned() {
        let cli = Cli::parse_from(["galvaan", "verify-unsigned", "copilot"]);
        match cli.command {
            Commands::VerifyUnsigned { name } => assert_eq!(name, "copilot"),
            _ => panic!("Expected VerifyUnsigned command"),
        }
    }

    #[test]
    fn test_cli_parse_add_with_explicit_pm() {
        let cli = Cli::parse_from([
            "galvaan",
            "add",
            "github/app",
            "--asset-pattern",
            "*.rpm",
            "--package-manager",
            "zypper",
        ]);
        match cli.command {
            Commands::Add {
                package_manager, ..
            } => {
                assert_eq!(package_manager.as_deref(), Some("zypper"));
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_cli_parse_add_without_asset_pattern() {
        let cli = Cli::parse_from(["galvaan", "add", "owner/repo"]);
        match cli.command {
            Commands::Add { asset_pattern, .. } => {
                assert!(asset_pattern.is_none());
            }
            _ => panic!("Expected Add command"),
        }
    }

    #[test]
    fn test_detect_asset_pattern_rpm() {
        let pattern = detect_asset_pattern(&PackageManagerType::Zypper);
        assert!(pattern.contains(".rpm"));
        assert!(pattern.contains("linux"));
    }

    #[test]
    fn test_detect_asset_pattern_deb() {
        let pattern = detect_asset_pattern(&PackageManagerType::Apt);
        assert!(pattern.contains(".deb"));
        assert!(pattern.contains("linux"));
    }

    #[test]
    fn test_detect_asset_pattern_pacman() {
        let pattern = detect_asset_pattern(&PackageManagerType::Pacman);
        assert!(pattern.contains(".pkg.tar.zst"));
        assert!(pattern.contains("linux"));
    }

    #[test]
    fn test_cli_parse_update_with_name() {
        let cli = Cli::parse_from(["galvaan", "update", "copilot"]);
        match cli.command {
            Commands::Update {
                name,
                version,
                prerelease,
            } => {
                assert_eq!(name.as_deref(), Some("copilot"));
                assert!(version.is_none());
                assert!(!prerelease);
            }
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parse_update_all() {
        let cli = Cli::parse_from(["galvaan", "update"]);
        match cli.command {
            Commands::Update {
                name,
                version,
                prerelease,
            } => {
                assert!(name.is_none());
                assert!(version.is_none());
                assert!(!prerelease);
            }
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parse_update_specific_version() {
        let cli = Cli::parse_from(["galvaan", "update", "copilot", "--version", "v1.0.24"]);
        match cli.command {
            Commands::Update { name, version, .. } => {
                assert_eq!(name.as_deref(), Some("copilot"));
                assert_eq!(version.as_deref(), Some("v1.0.24"));
            }
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parse_update_prerelease() {
        let cli = Cli::parse_from(["galvaan", "update", "--prerelease"]);
        match cli.command {
            Commands::Update { prerelease, .. } => assert!(prerelease),
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parse_check_prerelease() {
        let cli = Cli::parse_from(["galvaan", "check", "myapp", "--prerelease"]);
        match cli.command {
            Commands::Check { name, prerelease } => {
                assert_eq!(name.as_deref(), Some("myapp"));
                assert!(prerelease);
            }
            _ => panic!("Expected Check command"),
        }
    }

    #[test]
    fn test_cli_parse_pin() {
        let cli = Cli::parse_from(["galvaan", "pin", "copilot", "1.*"]);
        match cli.command {
            Commands::Pin { name, constraint } => {
                assert_eq!(name, "copilot");
                assert_eq!(constraint, "1.*");
            }
            _ => panic!("Expected Pin command"),
        }
    }

    #[test]
    fn test_cli_parse_unpin() {
        let cli = Cli::parse_from(["galvaan", "unpin", "copilot"]);
        match cli.command {
            Commands::Unpin { name } => assert_eq!(name, "copilot"),
            _ => panic!("Expected Unpin command"),
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
            Commands::Config {
                action: ConfigAction::Show,
            } => {}
            _ => panic!("Expected Config Show"),
        }
    }

    #[test]
    fn test_cli_parse_config_set() {
        let cli = Cli::parse_from(["galvaan", "config", "set", "auto_approve", "always"]);
        match cli.command {
            Commands::Config {
                action: ConfigAction::Set { key, value },
            } => {
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
            Commands::Config {
                action: ConfigAction::Path,
            } => {}
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
            Commands::Check { name, prerelease } => {
                assert_eq!(name.as_deref(), Some("copilot"));
                assert!(!prerelease);
            }
            _ => panic!("Expected Check command"),
        }
    }

    #[test]
    fn test_cli_parse_check_all() {
        let cli = Cli::parse_from(["galvaan", "check"]);
        match cli.command {
            Commands::Check { name, prerelease } => {
                assert!(name.is_none());
                assert!(!prerelease);
            }
            _ => panic!("Expected Check command"),
        }
    }

    #[test]
    fn test_install_options_from_config_defaults() {
        let config = Config::default();
        let opts = InstallOptions {
            auto_approve: config.settings.auto_approve.clone(),
            quiet: config.settings.quiet_package_manager,
            allow_unsigned: false,
            ignore_checksums: false,
        };
        assert_eq!(opts.auto_approve, AutoApprove::NoDeps);
        assert!(!opts.quiet);
        assert!(!opts.allow_unsigned);
    }

    #[test]
    fn test_install_options_cli_overrides() {
        let _config = Config::default();
        // Simulate CLI --auto-approve always -q
        let auto_approve = parse_auto_approve("always").unwrap();
        let opts = InstallOptions {
            auto_approve,
            quiet: true,
            allow_unsigned: false,
            ignore_checksums: false,
        };
        assert_eq!(opts.auto_approve, AutoApprove::Always);
        assert!(opts.quiet);
    }

    #[test]
    fn test_app_name_defaults_to_repo_name() {
        let app_name = default_app_name_from_source("github/app");
        assert_eq!(app_name, "app");
    }

    #[test]
    fn test_app_name_handles_no_slash() {
        let app_name = default_app_name_from_source("singlename");
        assert_eq!(app_name, "singlename");
    }

    #[test]
    fn test_app_name_defaults_to_url_filename() {
        let app_name =
            default_app_name_from_source("https://downloads.example.com/tool-v1.2.3-linux-x64.rpm");
        assert_eq!(app_name, "tool-v1.2.3-linux-x64.rpm");
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
        assert_eq!(
            config.settings.default_package_manager,
            PackageManagerType::Zypper
        );
    }
}
