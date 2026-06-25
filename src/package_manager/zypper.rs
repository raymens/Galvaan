use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

use super::{InstallOptions, PackageManager};
use crate::config::AutoApprove;

pub struct Zypper;

impl Zypper {
    /// Dry-run install to check whether new dependencies would be pulled in.
    /// Returns true if only the target package is affected (no extra deps).
    fn is_deps_only_update(&self, package_path: &str) -> Result<bool> {
        debug!("Running zypper dry-run to check for dependency changes");
        let output = Command::new("sudo")
            .args([
                "zypper",
                "install",
                "--dry-run",
                "--allow-unsigned-rpm",
                "-y",
                package_path,
            ])
            .output()
            .context("Failed to execute zypper dry-run")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!("Dry-run output:\n{stdout}");

        // Parse zypper dry-run output: look for "X new packages to install"
        // If only 1 package (the target) is being installed/upgraded, there are no new deps.
        let has_new_deps = stdout.lines().any(|line| {
            let line = line.trim();
            // "The following NEW packages are going to be installed:" indicates new deps
            line.contains("NEW packages are going to be installed")
        });

        Ok(!has_new_deps)
    }
}

impl PackageManager for Zypper {
    fn install(&self, package_path: &Path, options: &InstallOptions) -> Result<()> {
        let path_str = package_path
            .to_str()
            .context("Invalid package path encoding")?;

        info!("Installing {path_str} via zypper");

        let auto_yes = match &options.auto_approve {
            AutoApprove::Always => {
                debug!("Auto-approve: always — using -y");
                true
            }
            AutoApprove::NoDeps => {
                let no_new_deps = self.is_deps_only_update(path_str)?;
                if no_new_deps {
                    debug!("Auto-approve: no new dependencies detected — using -y");
                    println!("  No new dependencies — auto-approving.");
                    true
                } else {
                    warn!("New dependencies detected — prompting for approval");
                    println!("  New dependencies detected — prompting for approval.");
                    false
                }
            }
            AutoApprove::Never => {
                debug!("Auto-approve: never — prompting");
                false
            }
        };

        let mut args = vec!["zypper", "install", "--allow-unsigned-rpm"];
        if auto_yes {
            args.push("-y");
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
            .context("Failed to execute zypper install")?;

        if options.quiet {
            let stdout = String::from_utf8_lossy(&child.stdout);
            let stderr = String::from_utf8_lossy(&child.stderr);
            if !stdout.is_empty() {
                debug!("zypper stdout:\n{stdout}");
            }
            if !stderr.is_empty() {
                debug!("zypper stderr:\n{stderr}");
            }
        }

        if !child.status.success() {
            // Always show output on failure, even in quiet mode
            if options.quiet {
                let stderr = String::from_utf8_lossy(&child.stderr);
                if !stderr.is_empty() {
                    eprintln!("{stderr}");
                }
            }
            anyhow::bail!("zypper install failed with exit code: {:?}", child.status.code());
        }

        info!("Installation complete");
        if !options.quiet {
            println!("  Installation complete.");
        }
        Ok(())
    }

    fn installed_version(&self, package_name: &str) -> Result<Option<String>> {
        debug!("Querying installed version for {package_name}");
        let output = Command::new("rpm")
            .args(["-q", "--queryformat", "%{VERSION}", package_name])
            .output()
            .context("Failed to query rpm for installed version")?;

        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            debug!("Installed version: {version}");
            Ok(Some(version))
        } else {
            debug!("{package_name} is not installed");
            Ok(None)
        }
    }

    fn name(&self) -> &str {
        "zypper"
    }
}
