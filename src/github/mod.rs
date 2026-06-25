use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub name: Option<String>,
    pub prerelease: bool,
    pub draft: bool,
    pub assets: Vec<Asset>,
    pub html_url: String,
}

#[derive(Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
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

    /// Download an asset with a progress bar
    pub async fn download_asset(&self, url: &str, dest: &std::path::Path, total_size: u64) -> Result<()> {
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
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "GitHub-Copilot-linux-x64.rpm"));
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "GitHub-Copilot-*.rpm"));
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*-linux-x64.rpm"));
    }

    #[test]
    fn test_glob_matching_arch_filter() {
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*-x64.rpm"));
        assert!(!matches_pattern("GitHub-Copilot-linux-arm64.rpm", "*-x64.rpm"));
        assert!(matches_pattern("GitHub-Copilot-linux-arm64.rpm", "*-arm64.rpm"));
    }

    #[test]
    fn test_glob_matching_extension_filter() {
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*.rpm"));
        assert!(!matches_pattern("GitHub-Copilot-linux-x64.deb", "*.rpm"));
        assert!(matches_pattern("GitHub-Copilot-linux-x64.deb", "*.deb"));
        assert!(!matches_pattern("GitHub-Copilot-linux-x64.AppImage", "*.rpm"));
    }

    #[test]
    fn test_glob_exact_match() {
        assert!(matches_pattern("app.rpm", "app.rpm"));
        assert!(!matches_pattern("app.rpm", "other.rpm"));
    }

    #[test]
    fn test_glob_multiple_wildcards() {
        assert!(matches_pattern("GitHub-Copilot-linux-x64.rpm", "*Copilot*x64*"));
        assert!(!matches_pattern("GitHub-Copilot-linux-arm64.rpm", "*Copilot*x64*"));
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
        let matched_deb = assets.iter().find(|a| matches_pattern(&a.name, pattern_deb));
        assert!(matched_deb.is_some());
        assert_eq!(matched_deb.unwrap().name, "GitHub-Copilot-linux-x64.deb");

        let pattern_win = "*-windows-x64.msi";
        let matched_win = assets.iter().find(|a| matches_pattern(&a.name, pattern_win));
        assert!(matched_win.is_none());
    }
}
