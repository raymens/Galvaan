pub mod apt;
pub mod dnf;
pub mod pacman;
pub mod zypper;

use anyhow::Result;
use std::path::Path;

use crate::config::{AutoApprove, PackageManagerType};

/// Options passed to package manager operations
#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub auto_approve: AutoApprove,
    pub quiet: bool,
    /// Skip package signature verification (dangerous — only for unsigned packages)
    pub allow_unsigned: bool,
}

/// Trait for package manager implementations
#[allow(dead_code)]
pub trait PackageManager {
    /// Install a package from a local file path
    fn install(&self, package_path: &Path, options: &InstallOptions) -> Result<()>;

    /// Check if a package is already installed, return version if so
    fn installed_version(&self, package_name: &str) -> Result<Option<String>>;

    /// Name of this package manager
    fn name(&self) -> &str;
}

/// Create a package manager instance based on the type
pub fn create(pm_type: &PackageManagerType) -> Box<dyn PackageManager> {
    match pm_type {
        PackageManagerType::Zypper => Box::new(zypper::Zypper),
        PackageManagerType::Dnf => Box::new(dnf::Dnf),
        PackageManagerType::Apt => Box::new(apt::Apt),
        PackageManagerType::Pacman => Box::new(pacman::Pacman),
    }
}
