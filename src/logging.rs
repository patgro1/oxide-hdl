use std::fs::OpenOptions;
use std::io::Write;

// We need a unique, persistent file for the LSP crash logs
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
