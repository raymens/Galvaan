use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser)]
#[command(name = "galvaan")]
#[command(about = "Keep apps up to date based on GitHub releases")]
#[command(version)]
pub struct Cli {
    /// Override auto-approve setting (always, no-deps, never)
    #[arg(long, global = true)]
    pub auto_approve: Option<String>,

    /// Suppress package manager output
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Add a GitHub repository to track
    Add {
        /// GitHub repository (owner/repo format)
        repo: String,
        /// Friendly name for this app
        #[arg(short, long)]
        name: Option<String>,
        /// Glob pattern to match release assets (e.g. "*.x86_64.rpm")
        #[arg(short, long)]
        asset_pattern: String,
        /// Package manager to use (overrides default_package_manager from config)
        #[arg(short, long)]
        package_manager: Option<String>,
        /// Allow prerelease versions for this app
        #[arg(long)]
        prerelease: bool,
        /// Pin to a version constraint (e.g. "1.0.24", "1.*", ">=2.0.0,<3.0.0")
        #[arg(long)]
        pin: Option<String>,
        /// Skip package signature verification (for unsigned packages)
        #[arg(long)]
        allow_unsigned: bool,
    },

    /// Remove a tracked app
    Remove {
        /// Name of the app to remove
        name: String,
    },

    /// List all tracked apps
    List,

    /// Check for updates (all apps or a specific one)
    Check {
        /// Specific app name to check (checks all if omitted)
        name: Option<String>,
        /// Include prerelease versions (overrides per-app setting)
        #[arg(long)]
        prerelease: bool,
    },

    /// Update apps (all or a specific one)
    Update {
        /// Specific app name to update (updates all if omitted)
        name: Option<String>,
        /// Install a specific version instead of latest
        #[arg(long)]
        version: Option<String>,
        /// Include prerelease versions (overrides per-app setting)
        #[arg(long)]
        prerelease: bool,
    },

    /// Pin an app to a version constraint
    Pin {
        /// Name of the app to pin
        name: String,
        /// Version constraint (e.g. "1.0.24", "1.*", ">=2.0.0,<3.0.0")
        constraint: String,
    },

    /// Remove version pin from an app
    Unpin {
        /// Name of the app to unpin
        name: String,
    },

    /// View or modify configuration settings
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show current configuration
    Show,
    /// Set a configuration value
    Set {
        /// Setting key (auto_approve, default_package_manager, quiet_package_manager, log_file, log_level)
        key: String,
        /// Setting value
        value: String,
    },
    /// Show the config file path
    Path,
}

/// Generate shell completions and write to stdout
pub fn generate_completions(shell: Shell) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "galvaan", &mut std::io::stdout());
}
