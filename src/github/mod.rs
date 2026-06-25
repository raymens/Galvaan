use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Release {
    pub tag_name: String,
    #[allow(dead_code)]
    pub name: Option<String>,
    pub prerelease: bool,
    pub draft: bool,
    pub assets: Vec<Asset>,
    #[allow(dead_code)]
    pub html_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    #[allow(dead_code)]
    pub content_type: String,
}

pub struct GitHubClient {
    client: reqwest::Client,
}

impl GitHubClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("galvaan-updater/0.1.0")
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self { client })
    }

    /// Fetch the latest non-draft, non-prerelease release for a repo
    pub async fn get_latest_release(&self, repo: &str) -> Result<Release> {
        let url = format!("https://api.github.com/repos/{repo}/releases/latest");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch latest release for {repo}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error for {repo}: {status} - {body}");
        }

        let release: Release = response
            .json()
            .await
            .with_context(|| format!("Failed to parse release JSON for {repo}"))?;
        Ok(release)
    }

    /// Fetch recent releases (up to 100) for a repo, including prereleases
    pub async fn get_releases(&self, repo: &str, per_page: u32) -> Result<Vec<Release>> {
        let url = format!("https://api.github.com/repos/{repo}/releases?per_page={per_page}");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch releases for {repo}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API error for {repo}: {status} - {body}");
        }

        let releases: Vec<Release> = response
            .json()
            .await
            .with_context(|| format!("Failed to parse releases JSON for {repo}"))?;
        Ok(releases)
    }

    /// Fetch a specific release by tag name
    pub async fn get_release_by_tag(&self, repo: &str, tag: &str) -> Result<Release> {
        let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch release {tag} for {repo}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Release '{tag}' not found for {repo}: {status} - {body}");
        }

        let release: Release = response
            .json()
            .await
            .with_context(|| format!("Failed to parse release JSON for {repo}"))?;
        Ok(release)
    }

    /// Download an asset with a progress bar
    pub async fn download_asset(
        &self,
        url: &str,
        dest: &std::path::Path,
        total_size: u64,
    ) -> Result<()> {
        use futures_util::StreamExt;
        use indicatif::{ProgressBar, ProgressStyle};
        use tokio::io::AsyncWriteExt;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to download {url}"))?;

        if !response.status().is_success() {
            anyhow::bail!("Download failed with status: {}", response.status());
        }

        let content_length = response.content_length().unwrap_or(total_size);

        let pb = ProgressBar::new(content_length);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("  {bar:40.cyan/dim} {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                .expect("invalid progress bar template")
                .progress_chars("━╸─"),
        );

        let mut file = tokio::fs::File::create(dest)
            .await
            .with_context(|| format!("Failed to create {}", dest.display()))?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Error reading download stream")?;
            file.write_all(&chunk).await?;
            pb.inc(chunk.len() as u64);
        }

        file.flush().await?;
        pb.finish_and_clear();
        Ok(())
    }
}

/// Match an asset name against a glob-like pattern (supports * wildcard)
pub fn matches_pattern(name: &str, pattern: &str) -> bool {
    simple_glob_match(pattern, name)
}

/// Options for filtering releases when looking for the best candidate
pub struct ReleaseFilter<'a> {
    pub allow_prerelease: bool,
    pub version_pin: Option<&'a str>,
    pub specific_version: Option<&'a str>,
}

/// Find the best matching release from a list, applying prerelease and version pin filters.
/// Returns the first (newest) release that passes all filters.
pub fn find_best_release<'a>(
    releases: &'a [Release],
    filter: &ReleaseFilter<'_>,
) -> Option<&'a Release> {
    use crate::config::version_matches_pin;

    releases.iter().find(|r| {
        // Skip drafts always
        if r.draft {
            return false;
        }
        // Skip prereleases unless allowed
        if r.prerelease && !filter.allow_prerelease {
            return false;
        }
        // If specific version is requested, match exactly
        if let Some(target) = filter.specific_version {
            let tag = r.tag_name.trim_start_matches('v');
            let target_clean = target.trim_start_matches('v');
            return tag == target_clean || r.tag_name == target;
        }
        // Apply version pin constraint
        if let Some(pin) = filter.version_pin {
            return version_matches_pin(&r.tag_name, pin);
        }
        true
    })
}

fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(found) => {
                if i == 0 && found != 0 {
                    return false; // First part must match at start
                }
                pos += found + part.len();
            }
            None => return false,
        }
    }

    // If pattern doesn't end with *, the text must end at current pos
    if !pattern.ends_with('*') {
        return pos == text.len();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_matching_rpm() {
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*.rpm"));
        assert!(matches_pattern(
            "GitHub-Copilot-linux-x64.rpm",
            "GitHub-Copilot-linux-x64.rpm"
        ));
        assert!(matches_pattern(
            "GitHub-Copilot-linux-x64.rpm",
            "GitHub-Copilot-*.rpm"
        ));
        assert!(matches_pattern(
            "GitHub-Copilot-linux-x64.rpm",
            "*-linux-x64.rpm"
        ));
    }

    #[test]
    fn test_glob_matching_arch_filter() {
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*-x64.rpm"));
        assert!(!matches_pattern(
            "GitHub-Copilot-linux-arm64.rpm",
            "*-x64.rpm"
        ));
        assert!(matches_pattern(
            "GitHub-Copilot-linux-arm64.rpm",
            "*-arm64.rpm"
        ));
    }

    #[test]
    fn test_glob_matching_extension_filter() {
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*.rpm"));
        assert!(!matches_pattern("GitHub-Copilot-linux-x64.deb", "*.rpm"));
        assert!(matches_pattern("GitHub-Copilot-linux-x64.deb", "*.deb"));
        assert!(!matches_pattern(
            "GitHub-Copilot-linux-x64.AppImage",
            "*.rpm"
        ));
    }

    #[test]
    fn test_glob_exact_match() {
        assert!(matches_pattern("app.rpm", "app.rpm"));
        assert!(!matches_pattern("app.rpm", "other.rpm"));
    }

    #[test]
    fn test_glob_multiple_wildcards() {
        assert!(matches_pattern(
            "GitHub-Copilot-linux-x64.rpm",
            "*Copilot*x64*"
        ));
        assert!(!matches_pattern(
            "GitHub-Copilot-linux-arm64.rpm",
            "*Copilot*x64*"
        ));
    }

    #[test]
    fn test_glob_wildcard_at_start() {
        assert!(matches_pattern("something.rpm", "*.rpm"));
        assert!(matches_pattern(".rpm", "*.rpm"));
    }

    #[test]
    fn test_glob_no_match() {
        assert!(!matches_pattern("", "*.rpm"));
        assert!(!matches_pattern("file.txt", "*.rpm"));
    }

    #[test]
    fn test_release_deserialization() {
        let json = r#"{
            "tag_name": "v1.0.24",
            "name": "Release 1.0.24",
            "prerelease": false,
            "draft": false,
            "html_url": "https://github.com/github/app/releases/tag/v1.0.24",
            "assets": [
                {
                    "name": "GitHub-Copilot-linux-x64.rpm",
                    "browser_download_url": "https://github.com/github/app/releases/download/v1.0.24/GitHub-Copilot-linux-x64.rpm",
                    "size": 104857600,
                    "content_type": "application/x-rpm"
                },
                {
                    "name": "GitHub-Copilot-linux-x64.deb",
                    "browser_download_url": "https://github.com/github/app/releases/download/v1.0.24/GitHub-Copilot-linux-x64.deb",
                    "size": 104857600,
                    "content_type": "application/vnd.debian.binary-package"
                }
            ]
        }"#;

        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag_name, "v1.0.24");
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[0].name, "GitHub-Copilot-linux-x64.rpm");
        assert!(!release.prerelease);
        assert!(!release.draft);
    }

    #[test]
    fn test_find_matching_asset_in_release() {
        let assets = vec![
            Asset {
                name: "GitHub-Copilot-darwin-arm64.dmg".to_string(),
                browser_download_url: "https://example.com/mac.dmg".to_string(),
                size: 100,
                content_type: "application/octet-stream".to_string(),
            },
            Asset {
                name: "GitHub-Copilot-linux-x64.rpm".to_string(),
                browser_download_url: "https://example.com/linux.rpm".to_string(),
                size: 200,
                content_type: "application/x-rpm".to_string(),
            },
            Asset {
                name: "GitHub-Copilot-linux-x64.deb".to_string(),
                browser_download_url: "https://example.com/linux.deb".to_string(),
                size: 200,
                content_type: "application/vnd.debian.binary-package".to_string(),
            },
        ];

        let pattern = "*-linux-x64.rpm";
        let matched = assets.iter().find(|a| matches_pattern(&a.name, pattern));
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().name, "GitHub-Copilot-linux-x64.rpm");

        let pattern_deb = "*-linux-x64.deb";
        let matched_deb = assets
            .iter()
            .find(|a| matches_pattern(&a.name, pattern_deb));
        assert!(matched_deb.is_some());
        assert_eq!(matched_deb.unwrap().name, "GitHub-Copilot-linux-x64.deb");

        let pattern_win = "*-windows-x64.msi";
        let matched_win = assets
            .iter()
            .find(|a| matches_pattern(&a.name, pattern_win));
        assert!(matched_win.is_none());
    }

    // ─── find_best_release tests ─────────────────────────────────────────

    fn make_release(tag: &str, prerelease: bool, draft: bool) -> Release {
        Release {
            tag_name: tag.to_string(),
            name: Some(tag.to_string()),
            prerelease,
            draft,
            assets: vec![],
            html_url: format!("https://github.com/test/repo/releases/tag/{tag}"),
        }
    }

    #[test]
    fn test_find_best_release_stable_only() {
        let releases = vec![
            make_release("v2.0.0-beta.1", true, false),
            make_release("v1.5.0", false, false),
            make_release("v1.4.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: false,
            version_pin: None,
            specific_version: None,
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v1.5.0");
    }

    #[test]
    fn test_find_best_release_with_prerelease() {
        let releases = vec![
            make_release("v2.0.0-beta.1", true, false),
            make_release("v1.5.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: true,
            version_pin: None,
            specific_version: None,
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v2.0.0-beta.1");
    }

    #[test]
    fn test_find_best_release_skips_drafts() {
        let releases = vec![
            make_release("v3.0.0", false, true), // draft
            make_release("v2.0.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: true,
            version_pin: None,
            specific_version: None,
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v2.0.0");
    }

    #[test]
    fn test_find_best_release_with_version_pin() {
        let releases = vec![
            make_release("v2.1.0", false, false),
            make_release("v2.0.0", false, false),
            make_release("v1.9.0", false, false),
            make_release("v1.5.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: false,
            version_pin: Some("1.*"),
            specific_version: None,
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v1.9.0");
    }

    #[test]
    fn test_find_best_release_with_exact_pin() {
        let releases = vec![
            make_release("v2.0.0", false, false),
            make_release("v1.5.0", false, false),
            make_release("v1.0.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: false,
            version_pin: Some("1.5.0"),
            specific_version: None,
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v1.5.0");
    }

    #[test]
    fn test_find_best_release_specific_version() {
        let releases = vec![
            make_release("v2.0.0", false, false),
            make_release("v1.5.0", false, false),
            make_release("v1.0.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: false,
            version_pin: None,
            specific_version: Some("v1.0.0"),
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v1.0.0");
    }

    #[test]
    fn test_find_best_release_specific_version_without_v() {
        let releases = vec![
            make_release("v2.0.0", false, false),
            make_release("v1.5.0", false, false),
        ];
        let filter = ReleaseFilter {
            allow_prerelease: false,
            version_pin: None,
            specific_version: Some("1.5.0"),
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v1.5.0");
    }

    #[test]
    fn test_find_best_release_no_match() {
        let releases = vec![make_release("v2.0.0", false, false)];
        let filter = ReleaseFilter {
            allow_prerelease: false,
            version_pin: Some("1.*"),
            specific_version: None,
        };
        assert!(find_best_release(&releases, &filter).is_none());
    }

    #[test]
    fn test_find_best_release_prerelease_with_pin() {
        let releases = vec![
            make_release("v2.0.0-rc.1", true, false),
            make_release("v1.9.0", false, false),
            make_release("v1.5.0-beta.2", true, false),
            make_release("v1.5.0-beta.1", true, false),
        ];
        // Allow prereleases pinned to 1.*
        let filter = ReleaseFilter {
            allow_prerelease: true,
            version_pin: Some("1.*"),
            specific_version: None,
        };
        let best = find_best_release(&releases, &filter).unwrap();
        assert_eq!(best.tag_name, "v1.9.0");
    }
}
