//! Running as a login-time background program.
//!
//! Design: `docs/design/09-install-recover-migrate.md`, "常驻": after
//! installation it is a program that starts when the user logs in and keeps
//! running; the menu bar icon comes with the application shell. Until then,
//! this installs the daemon as a launchd agent for the current user, which
//! is the mechanism the shell will use as well.
//!
//! The agent runs `genatrix serve` with the data directory it was installed
//! from, restarts it if it stops, and writes its log beside the data. It
//! reads mail passwords from the keychain, so nothing secret is in the
//! property list.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The launchd label. Reverse-DNS by convention; the domain is the
/// project's.
pub const LABEL: &str = "xyz.dpt.genatrix";

/// Where the agent's property list lives for this user.
pub fn plist_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// The property list for one installation.
#[must_use]
pub fn plist(binary: &Path, data_dir: &Path, port: u16, log: &Path) -> String {
    let arg = |p: &Path| escape(&p.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--data-dir</string>
    <string>{}</string>
    <string>serve</string>
    <string>--port</string>
    <string>{port}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        arg(binary),
        arg(data_dir),
        arg(log),
        arg(log),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd domain for the current user's session.
fn domain() -> String {
    // `id -u` rather than a libc call: this file has no other reason to
    // take on a dependency for one number.
    let uid = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "501".to_owned(), |s| s.trim().to_owned());
    format!("gui/{uid}")
}

/// Write the property list and start the agent. Replaces an existing one.
pub fn install(data_dir: &Path, port: u16) -> anyhow::Result<PathBuf> {
    let binary = std::env::current_exe()?.canonicalize()?;
    let log_dir = data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Out with the old first, quietly: there may be none.
    let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain())]);
    std::fs::write(
        &path,
        plist(&binary, data_dir, port, &log_dir.join("genatrix.log")),
    )?;
    launchctl(&["bootstrap", &domain(), &path.display().to_string()])?;
    Ok(path)
}

/// Stop the agent and remove its property list.
pub fn uninstall() -> anyhow::Result<bool> {
    let path = plist_path()?;
    let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain())]);
    if path.exists() {
        std::fs::remove_file(&path)?;
        return Ok(true);
    }
    Ok(false)
}

/// Whether launchd currently knows the agent.
pub fn installed() -> bool {
    launchctl(&["print", &format!("{}/{LABEL}", domain())]).is_ok()
}

fn launchctl(args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new("/bin/launchctl").args(args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_property_list_names_the_binary_the_data_and_the_log() {
        let text = plist(
            Path::new("/opt/genatrix/bin/genatrix"),
            Path::new("/Users/someone/Library/Application Support/Genatrix"),
            47600,
            Path::new("/Users/someone/Library/Application Support/Genatrix/logs/genatrix.log"),
        );
        assert!(text.contains("<string>/opt/genatrix/bin/genatrix</string>"));
        assert!(text.contains("<string>--data-dir</string>"));
        assert!(text.contains("<string>serve</string>"));
        assert!(text.contains("<string>47600</string>"));
        assert!(text.contains("<key>KeepAlive</key>\n  <true/>"));
        assert!(
            !text.contains("PASSWORD"),
            "nothing secret goes in the plist"
        );
    }

    #[test]
    fn paths_with_markup_characters_are_escaped() {
        let text = plist(
            Path::new("/tmp/a&b/genatrix"),
            Path::new("/tmp/<data>"),
            1,
            Path::new("/tmp/log"),
        );
        assert!(text.contains("/tmp/a&amp;b/genatrix"));
        assert!(text.contains("/tmp/&lt;data&gt;"));
    }
}
