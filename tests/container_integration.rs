//! Integration tests that run galvaan inside real distro containers using Podman/Docker.
//!
//! These tests are marked `#[ignore]` so they don't run during normal `cargo test`.
//! Run them explicitly with:
//!
//!   cargo test --test container_integration -- --ignored
//!
//! Or run a single distro:
//!
//!   cargo test --test container_integration opensuse_tumbleweed -- --ignored
//!
//! Prerequisites:
//!   - podman (preferred) or docker
//!   - network access (pulls base images + hits GitHub API)

use std::path::PathBuf;
use std::process::{Command, Output};

// ─── Container runtime detection ─────────────────────────────────────────────

fn container_runtime() -> &'static str {
    // Prefer podman — it's rootless and standard on openSUSE
    for rt in &["podman", "docker"] {
        if Command::new(rt)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return rt;
        }
    }
    panic!("Neither podman nor docker found. Install one to run integration tests.");
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn binary_path() -> PathBuf {
    // Build in release for smaller image and realistic performance
    let path = project_root().join("target/release/galvaan");
    if !path.exists() {
        panic!(
            "Release binary not found at {}. Run `cargo build --release` first.",
            path.display()
        );
    }
    path
}

// ─── Distro test harness ─────────────────────────────────────────────────────

struct DistroTest {
    name: &'static str,
    containerfile: &'static str,
    /// The package manager expected on this distro
    package_manager: &'static str,
}

impl DistroTest {
    fn image_tag(&self) -> String {
        format!("galvaan-integration-{}", self.name)
    }

    fn containerfile_path(&self) -> PathBuf {
        project_root()
            .join("tests/integration/distros")
            .join(self.containerfile)
    }

    fn test_script_path(&self) -> PathBuf {
        project_root().join("tests/integration/test_in_container.sh")
    }

    /// Build the container image with the galvaan binary baked in.
    fn build_image(&self) -> Output {
        let runtime = container_runtime();
        let binary = binary_path();
        let containerfile = self.containerfile_path();

        // Use a per-distro filename to avoid races when tests run in parallel
        let context_dir = containerfile.parent().unwrap();
        let binary_name = format!("galvaan-{}", self.name);
        let context_binary = context_dir.join(&binary_name);
        std::fs::copy(&binary, &context_binary).unwrap_or_else(|e| {
            panic!(
                "Failed to copy binary {} -> {}: {}",
                binary.display(),
                context_binary.display(),
                e
            )
        });

        let output = Command::new(runtime)
            .args([
                "build",
                "--no-cache",
                "-t",
                &self.image_tag(),
                "--build-arg",
                &format!("BINARY={}", binary_name),
                "-f",
                containerfile.to_str().unwrap(),
                context_dir.to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute container build");

        // Clean up copied binary
        let _ = std::fs::remove_file(&context_binary);

        output
    }

    /// Run the test script inside the container.
    fn run_tests(&self) -> Output {
        let runtime = container_runtime();
        let test_script = self.test_script_path();

        Command::new(runtime)
            .args([
                "run",
                "--rm",
                "--security-opt",
                "label=disable",
                "-e",
                &format!("GALVAAN_TEST_PM={}", self.package_manager),
                "-v",
                &format!("{}:/tests/test_in_container.sh:ro", test_script.display()),
                &self.image_tag(),
                "bash",
                "/tests/test_in_container.sh",
            ])
            .output()
            .expect("Failed to execute container run")
    }

    /// Clean up the image after tests.
    fn cleanup(&self) {
        let runtime = container_runtime();
        let _ = Command::new(runtime)
            .args(["rmi", "-f", &self.image_tag()])
            .output();
    }
}

fn run_distro_test(distro: &DistroTest) {
    println!("\n{}", "=".repeat(60));
    println!("Integration test: {}", distro.name);
    println!("{}\n", "=".repeat(60));

    // Build
    println!("Building container image...");
    let build_output = distro.build_image();
    let build_stderr = String::from_utf8_lossy(&build_output.stderr);
    let build_stdout = String::from_utf8_lossy(&build_output.stdout);

    if !build_output.status.success() {
        distro.cleanup();
        panic!(
            "Container build failed for {}:\nstdout:\n{}\nstderr:\n{}",
            distro.name, build_stdout, build_stderr
        );
    }
    println!("Image built successfully.");

    // Run tests
    println!("Running tests in container...");
    let test_output = distro.run_tests();
    let test_stdout = String::from_utf8_lossy(&test_output.stdout);
    let test_stderr = String::from_utf8_lossy(&test_output.stderr);

    println!("{}", test_stdout);
    if !test_stderr.is_empty() {
        eprintln!("{}", test_stderr);
    }

    // Cleanup
    distro.cleanup();

    assert!(
        test_output.status.success(),
        "Integration tests FAILED for {}. See output above.",
        distro.name,
    );
}

// ─── Distro definitions ──────────────────────────────────────────────────────
//
// Add new distros here. Each gets its own #[test] function so they can run
// independently and report results per-distro.

const OPENSUSE_TUMBLEWEED: DistroTest = DistroTest {
    name: "opensuse-tumbleweed",
    containerfile: "Containerfile.opensuse-tumbleweed",
    package_manager: "zypper",
};

const FEDORA: DistroTest = DistroTest {
    name: "fedora",
    containerfile: "Containerfile.fedora",
    package_manager: "dnf",
};

const UBUNTU: DistroTest = DistroTest {
    name: "ubuntu",
    containerfile: "Containerfile.ubuntu",
    package_manager: "apt",
};

const ARCHLINUX: DistroTest = DistroTest {
    name: "archlinux",
    containerfile: "Containerfile.archlinux",
    package_manager: "pacman",
};

// ─── Test functions ──────────────────────────────────────────────────────────

#[test]
#[ignore]
fn integration_opensuse_tumbleweed() {
    run_distro_test(&OPENSUSE_TUMBLEWEED);
}

#[test]
#[ignore]
fn integration_fedora() {
    run_distro_test(&FEDORA);
}

#[test]
#[ignore]
fn integration_ubuntu() {
    run_distro_test(&UBUNTU);
}

#[test]
#[ignore]
fn integration_archlinux() {
    run_distro_test(&ARCHLINUX);
}
