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
    fn is_deps_only_update(
        &self,
        package_path: &str,
        allow_unsigned: bool,
        ignore_checksums: bool,
    ) -> Result<bool> {
        debug!("Running zypper dry-run to check for dependency changes");
        let args =
            Self::build_install_args(package_path, allow_unsigned, ignore_checksums, true, true);

        let output = Command::new("sudo")
            .args(&args)
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

    fn build_install_args(
        package_path: &str,
        allow_unsigned: bool,
        ignore_checksums: bool,
        dry_run: bool,
        auto_yes: bool,
    ) -> Vec<String> {
        let mut args = vec!["zypper".to_string()];
        if ignore_checksums {
            args.push("--no-gpg-checks".to_string());
        }
        args.push("install".to_string());
        if dry_run {
            args.push("--dry-run".to_string());
        }
        if allow_unsigned {
            args.push("--allow-unsigned-rpm".to_string());
        }
        if auto_yes {
            args.push("-y".to_string());
        }
        args.push(package_path.to_string());
        args
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
                let no_new_deps = self.is_deps_only_update(
                    path_str,
                    options.allow_unsigned,
                    options.ignore_checksums,
                )?;
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

        let args = Self::build_install_args(
            path_str,
            options.allow_unsigned,
            options.ignore_checksums,
            false,
            auto_yes,
        );

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
            anyhow::bail!(
                "zypper install failed with exit code: {:?}",
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

#[cfg(test)]
mod tests {
    use super::Zypper;

    #[test]
    fn build_install_args_includes_no_gpg_checks_when_unsigned_checks_are_disabled() {
        let args = Zypper::build_install_args("/tmp/app.rpm", true, true, false, true);
        assert_eq!(
            args,
            vec![
                "zypper",
                "--no-gpg-checks",
                "install",
                "--allow-unsigned-rpm",
                "-y",
                "/tmp/app.rpm",
            ]
        );
    }

    #[test]
    fn build_install_args_omits_gpg_bypass_flags_by_default() {
        let args = Zypper::build_install_args("/tmp/app.rpm", false, false, false, false);
        assert_eq!(args, vec!["zypper", "install", "/tmp/app.rpm"]);
    }

    #[test]
    fn build_dry_run_args_include_dry_run_and_unsigned_flags() {
        let args = Zypper::build_install_args("/tmp/app.rpm", true, true, true, true);
        assert_eq!(
            args,
            vec![
                "zypper",
                "--no-gpg-checks",
                "install",
                "--dry-run",
                "--allow-unsigned-rpm",
                "-y",
                "/tmp/app.rpm",
            ]
        );
    }
}
