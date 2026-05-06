use std::process::Command;

/// Get the idle time in seconds since last user input (keyboard/mouse/trackpad).
/// Uses macOS IOKit HIDIdleTime property.
pub fn get_idle_seconds() -> Result<f64, Box<dyn std::error::Error>> {
    let output = Command::new("ioreg")
        .args(["-c", "IOHIDSystem", "-d", "4"])
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;

    for line in stdout.lines() {
        if line.contains("HIDIdleTime") {
            // Format: "HIDIdleTime" = 1234567890
            if let Some(val_str) = line.split('=').nth(1) {
                let val_str = val_str.trim();
                let nanos: u64 = val_str.parse()?;
                return Ok(nanos as f64 / 1_000_000_000.0);
            }
        }
    }

    Err("Could not find HIDIdleTime in ioreg output".into())
}
