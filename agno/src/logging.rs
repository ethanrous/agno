//! Unified logging configuration for agno.
//!
//! Provides structured logging using the `tracing` ecosystem with support for
//! both human-readable (CLI) and JSON (library) output formats.

use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

/// Output format for log messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable format for CLI usage.
    Human,
    /// JSON format for programmatic parsing.
    Json,
}

/// Logging configuration.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Output format (human-readable or JSON).
    pub format: LogFormat,
    /// Whether to include timestamps.
    pub timestamps: bool,
    /// Default log level.
    pub level: Level,
    /// Whether to include file/line information.
    pub file_info: bool,
    /// Whether to include target (module path).
    pub target: bool,
}

impl LogConfig {
    /// Create a configuration suitable for CLI usage.
    ///
    /// - Human-readable format
    /// - No timestamps (keeps output clean)
    /// - Warn level by default (can be overridden via RUST_LOG)
    pub fn cli() -> Self {
        Self {
            format: LogFormat::Human,
            timestamps: false,
            level: Level::WARN,
            file_info: false,
            target: false,
        }
    }

    /// Create a configuration suitable for library usage.
    ///
    /// - JSON format for programmatic parsing
    /// - Includes timestamps
    /// - Info level by default
    #[allow(dead_code)]
    pub fn library() -> Self {
        Self {
            format: LogFormat::Json,
            timestamps: true,
            level: Level::INFO,
            file_info: true,
            target: true,
        }
    }

    /// Create a configuration for debug output.
    ///
    /// - Human-readable format
    /// - Includes timestamps and file info
    /// - Debug level
    #[allow(dead_code)]
    pub fn debug() -> Self {
        Self {
            format: LogFormat::Human,
            timestamps: true,
            level: Level::DEBUG,
            file_info: true,
            target: true,
        }
    }
}

/// Initialize the logging subsystem with the given configuration.
///
/// # Panics
///
/// Panics if the logging subsystem has already been initialized.
pub fn init(config: LogConfig) {
    try_init(config).expect("Failed to initialize logging");
}

/// Try to initialize the logging subsystem with the given configuration.
///
/// Returns `Ok(())` if successful, or `Err` if already initialized.
/// This is safe to call multiple times - subsequent calls will be no-ops.
///
/// The log level can be overridden via the `AGNO_LOG_LEVEL` environment variable.
/// Examples:
/// - `AGNO_LOG_LEVEL=debug` - debug level for all modules
/// - `AGNO_LOG_LEVEL=warn,agno=debug` - warn for dependencies, debug for agno
pub fn try_init(config: LogConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Build default filter: suppress dependencies (error only), show agno at configured level
    let level_str = match config.level {
        Level::TRACE => "trace",
        Level::DEBUG => "debug",
        Level::INFO => "info",
        Level::WARN => "warn",
        Level::ERROR => "error",
    };
    let default_filter = format!("error,agno={}", level_str);

    // Check for AGNO_LOG_LEVEL env override, fallback to default filter
    let env_filter = match std::env::var("AGNO_LOG_LEVEL") {
        Ok(val) if !val.is_empty() => {
            EnvFilter::try_new(format!("error,agno={}", val)).unwrap_or_else(|_| {
                // If user's filter is invalid, use our default
                EnvFilter::try_new(&default_filter).unwrap_or_else(|_| EnvFilter::default())
            })
        }
        _ => EnvFilter::try_new(&default_filter).unwrap_or_else(|_| EnvFilter::default()),
    };

    // Get formatting layer based on config or environment (AGNO_LOG_FORMAT)
    let format = std::env::var("AGNO_LOG_FORMAT")
        .ok()
        .and_then(|s| match s.to_lowercase().as_str() {
            "human" => Some(LogFormat::Human),
            "json" => Some(LogFormat::Json),
            _ => None,
        })
        .unwrap_or(config.format);

    match format {
        LogFormat::Human => {
            let fmt_layer = fmt::layer()
                .with_ansi(true)
                .with_target(config.target)
                .with_file(config.file_info)
                .with_line_number(config.file_info)
                .with_span_events(FmtSpan::NONE);

            let fmt_layer = if config.timestamps {
                fmt_layer.boxed()
            } else {
                fmt_layer.without_time().boxed()
            };

            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .try_init()?;
        }
        LogFormat::Json => {
            let json_layer = fmt::layer()
                .json()
                .with_target(config.target)
                .with_file(config.file_info)
                .with_line_number(config.file_info)
                .with_span_events(FmtSpan::NONE);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(json_layer)
                .try_init()?;
        }
    }

    Ok(())
}
