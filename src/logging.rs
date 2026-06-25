use anyhow::{Context, Result};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;

pub fn init(config: &Config) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let log_level = &config.settings.log_level;

    match &config.settings.log_file {
        Some(log_path) => {
            let path = std::path::Path::new(log_path);
            let parent = path.parent().context("Invalid log file path")?;
            std::fs::create_dir_all(parent)?;

            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("Failed to open log file: {log_path}"))?;

            let (non_blocking, guard) = tracing_appender::non_blocking(file);

            let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info"));

            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false)
                        .with_target(true)
                        .with_thread_ids(false),
                )
                .init();

            Ok(Some(guard))
        }
        None => {
            // No file logging configured — only init if RUST_LOG is set
            if std::env::var("RUST_LOG").is_ok() {
                tracing_subscriber::registry()
                    .with(EnvFilter::from_default_env())
                    .with(fmt::layer().with_writer(std::io::stderr))
                    .init();
            }
            Ok(None)
        }
    }
}
