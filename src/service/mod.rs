use std::path::PathBuf;

const PLIST_LABEL: &str = "com.sleepyminer.miner";

pub fn plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", PLIST_LABEL))
}

pub fn install_service(config_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let binary = std::env::current_exe()?;
    let binary_str = binary.to_string_lossy();

    let log_dir = crate::config::dirs_path().join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let stdout_log = log_dir.join("sleepyminer.log");
    let stderr_log = log_dir.join("sleepyminer.err");

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--config</string>
        <string>{config}</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>
    <key>Nice</key>
    <integer>10</integer>
</dict>
</plist>"#,
        label = PLIST_LABEL,
        binary = binary_str,
        config = config_path,
        stdout = stdout_log.to_string_lossy(),
        stderr = stderr_log.to_string_lossy(),
    );

    let path = plist_path();
    std::fs::write(&path, plist_content)?;

    println!("Service installed at: {}", path.display());
    println!("Loading service...");

    let status = std::process::Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .status()?;

    if status.success() {
        println!("Service loaded successfully. Sleepyminer will start on login.");
    } else {
        println!("Warning: launchctl load returned non-zero exit code.");
    }

    Ok(())
}

pub fn uninstall_service() -> Result<(), Box<dyn std::error::Error>> {
    let path = plist_path();

    if path.exists() {
        println!("Unloading service...");
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()])
            .status();

        std::fs::remove_file(&path)?;
        println!("Service removed: {}", path.display());
    } else {
        println!("No service installed at: {}", path.display());
    }

    Ok(())
}
