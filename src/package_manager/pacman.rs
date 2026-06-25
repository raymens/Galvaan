use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, info};

use super::{InstallOptions, PackageManager};
use crate::config::AutoApprove;

pub struct Pacman;

impl PackageManager for Pacman {
    fn install(&self, package_path: &Path, options: &InstallOptions) -> Result<()> {
        let path_str = package_path
            .to_str()
            .context("Invalid package path encoding")?;

        info!("Installing {path_str} via pacman");

        // pacman -U installs local packages
        // --noconfirm is the equivalent of -y
        let auto_yes = match &options.auto_approve {
            AutoApprove::Always | AutoApprove::NoDeps => {
                // pacman doesn't have a convenient dry-run for dep checking,
                // so NoDeps behaves like Always for now
                debug!("Auto-approve: using --noconfirm");
                true
            }
            AutoApprove::Never => {
                debug!("Auto-approve: never — prompting");
                false
            }
        };

        let mut args = vec!["pacman", "-U"];
        if auto_yes {
            args.push("--noconfirm");
        }
        args.push(path_str);

        let (stdout_cfg, stderr_cfg) = if options.quiet {
            debug!("Quiet mode: suppressing package manager output");
            (Stdio::piped(), Stdio::piped())
        } else {
            (Stdio::inherit(), Stdio::inherit())
        };

        let child = Command::new("sudo")
            .args(&args)
            .stdout(stdout_cfg)
            .stderr(stderr_cfg)
            .output()
            .context("Failed to execute pacman -U")?;

        if options.quiet {
            let stdout = String::from_utf8_lossy(&child.stdout);
            let stderr = String::from_utf8_lossy(&child.stderr);
            if !stdout.is_empty() {
                debug!("pacman stdout:\n{stdout}");
            }
            if !stderr.is_empty() {
                debug!("pacman stderr:\n{stderr}");
            }
        }

        if !child.status.success() {
            if options.quiet {
                let stderr = String::from_utf8_lossy(&child.stderr);
                if !stderr.is_empty() {
                    eprintln!("{stderr}");
                }
            }
            anyhow::bail!(
                "pacman install failed with exit code: {:?}",
                child.status.code()
            );
        }

        info!("Installation complete");
        if !options.quiet {
            println!("  Installation complete.");
        }
        Ok(())
    }

    fn installed_version(&self, package_name: &str) -> Result<Option<String>> {
        debug!("Querying installed version for {package_name}");
        let output = Command::new("pacman")
            .args(["-Q", package_name])
            .output()
            .context("Failed to query pacman for installed version")?;

        if output.status.success() {
            // pacman -Q outputs "package_name version"
            let stdout = String::from_utf8_lossy(&output.stdout);
            let version = stdout.split_whitespace().nth(1).unwrap_or("").to_string();
            debug!("Installed version: {version}");
            Ok(Some(version))
        } else {
            debug!("{package_name} is not installed");
            Ok(None)
        }
    }

    fn name(&self) -> &str {
        "pacman"
    }
}
