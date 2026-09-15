use anyhow::{Context, Result};
use reqwest::header::{CONTENT_DISPOSITION, ETAG, LAST_MODIFIED, RANGE};

#[derive(Debug, Clone)]
pub struct UrlProbe {
    pub identity: String,
    pub resolved_url: String,
    #[allow(dead_code)]
    pub etag: Option<String>,
    #[allow(dead_code)]
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    pub filename: Option<String>,
}

pub struct UrlSourceClient {
    client: reqwest::Client,
}

impl UrlSourceClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("galvaan-updater/0.1.0")
            .build()
            .context("Failed to build HTTP client")?;
        Ok(Self { client })
    }

    pub async fn probe_identity(&self, url: &str) -> Result<UrlProbe> {
        let mut response = self
            .client
            .head(url)
            .send()
            .await
            .with_context(|| format!("Failed to probe URL metadata for {url}"))?;

        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED
            || response.status() == reqwest::StatusCode::NOT_IMPLEMENTED
        {
            // Some servers reject HEAD; request a tiny range to get headers.
            response = self
                .client
                .get(url)
                .header(RANGE, "bytes=0-0")
                .send()
                .await
                .with_context(|| format!("Failed fallback metadata probe for {url}"))?;
        }

        if !(response.status().is_success() || response.status() == reqwest::StatusCode::PARTIAL_CONTENT)
        {
            anyhow::bail!("URL probe failed for {url}: {}", response.status());
        }

        let headers = response.headers();
        let resolved_url = response.url().to_string();
        let etag = header_to_string(headers.get(ETAG));
        let last_modified = header_to_string(headers.get(LAST_MODIFIED));
        let content_length = response
            .content_length()
            .or_else(|| parse_u64_header(headers.get(reqwest::header::CONTENT_LENGTH)));
        let filename = parse_filename_from_content_disposition(headers.get(CONTENT_DISPOSITION))
            .or_else(|| filename_from_url(response.url()));

        Ok(UrlProbe {
            identity: compute_identity(etag.as_deref(), last_modified.as_deref(), &resolved_url),
            resolved_url,
            etag,
            last_modified,
            content_length,
            filename,
        })
    }

    pub async fn download_package(
        &self,
        url: &str,
        dest: &std::path::Path,
        expected_size: Option<u64>,
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

        let total_size = response.content_length().or(expected_size).unwrap_or(0);
        let pb = if total_size > 0 {
            let pb = ProgressBar::new(total_size);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("  {bar:40.cyan/dim} {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
                    .expect("invalid progress bar template")
                    .progress_chars("#>-"),
            );
            pb
        } else {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            pb.set_message("downloading...");
            pb
        };

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

pub fn compute_identity(etag: Option<&str>, last_modified: Option<&str>, resolved_url: &str) -> String {
    if let Some(v) = etag {
        return format!("etag:{}", v.trim());
    }
    if let Some(v) = last_modified {
        return format!("last-modified:{}", v.trim());
    }
    format!("url:{resolved_url}")
}

pub fn should_update_for_identity(stored_identity: Option<&str>, current_identity: &str) -> bool {
    match stored_identity {
        Some(stored) => stored != current_identity,
        None => true,
    }
}

pub fn best_effort_version_from_filename(filename: &str) -> Option<String> {
    let stem = filename
        .rsplit('/')
        .next()
        .unwrap_or(filename)
        .split('?')
        .next()
        .unwrap_or(filename)
        .split('#')
        .next()
        .unwrap_or(filename)
        .trim();

    if stem.is_empty() {
        return None;
    }

    let cleaned = stem
        .trim_end_matches(".pkg.tar.zst")
        .trim_end_matches(".tar.gz")
        .trim_end_matches(".rpm")
        .trim_end_matches(".deb")
        .trim_end_matches(".pkg");

    for part in cleaned.split(['-', '_']) {
        let candidate = part.trim_start_matches('v');
        if candidate.chars().any(|c| c.is_ascii_digit()) && candidate.contains('.') {
            return Some(candidate.to_string());
        }
    }

    None
}

fn header_to_string(value: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    value.and_then(|v| v.to_str().ok()).map(|v| v.to_string())
}

fn parse_u64_header(value: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    value
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

fn parse_filename_from_content_disposition(
    value: Option<&reqwest::header::HeaderValue>,
) -> Option<String> {
    let raw = value?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(name) = part.strip_prefix("filename=") {
            return Some(name.trim_matches('"').to_string());
        }
    }
    None
}

fn filename_from_url(url: &reqwest::Url) -> Option<String> {
    url.path_segments()?
        .next_back()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_identity_prefers_etag() {
        let id = compute_identity(Some("\"abc\""), Some("Mon, 01 Jan 2024 00:00:00 GMT"), "https://x/y");
        assert_eq!(id, "etag:\"abc\"");
    }

    #[test]
    fn test_compute_identity_uses_last_modified_when_no_etag() {
        let id = compute_identity(None, Some("Mon, 01 Jan 2024 00:00:00 GMT"), "https://x/y");
        assert_eq!(id, "last-modified:Mon, 01 Jan 2024 00:00:00 GMT");
    }

    #[test]
    fn test_compute_identity_falls_back_to_url() {
        let id = compute_identity(None, None, "https://example.com/tool.rpm");
        assert_eq!(id, "url:https://example.com/tool.rpm");
    }

    #[test]
    fn test_should_update_for_identity() {
        assert!(should_update_for_identity(None, "etag:a"));
        assert!(!should_update_for_identity(Some("etag:a"), "etag:a"));
        assert!(should_update_for_identity(Some("etag:a"), "etag:b"));
    }

    #[test]
    fn test_best_effort_version_from_filename() {
        assert_eq!(
            best_effort_version_from_filename("tool-v1.2.3-linux-x64.rpm").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            best_effort_version_from_filename("https://example.com/releases/tool_2.0.1_amd64.deb")
                .as_deref(),
            Some("2.0.1")
        );
    }
}

