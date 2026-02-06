//! Logging utilities for Oxide HDL.
//!
//! Provides crash logging functionality for debugging LSP issues.

use std::fs::OpenOptions;
use std::io::Write;

/// Logs a crash message to a persistent file for debugging.
///
/// Writes timestamped messages to `/tmp/oxide_hdl_crash.log` for post-mortem
/// analysis of LSP crashes or unexpected errors.
///
/// # Arguments
///
/// * `msg` - The message to log.
#[allow(dead_code)]
pub fn log_crash(msg: &str) {
    let path = "/tmp/oxide_hdl_crash.log";
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            let timestamp = chrono::Local::now().format("%H:%M:%S");
            let _ = writeln!(file, "[{}] {}", timestamp, msg);
        }
        Err(e) => {
            // If we can't even write the log, something is seriously wrong!
            eprintln!("CRITICAL: Failed to write to log file: {}", e);
        }
    }
}
