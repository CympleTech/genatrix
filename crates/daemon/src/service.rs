//! Running as a login-time background program.
//!
//! Design: `docs/design/09-install-recover-migrate.md`, "常驻": after
//! installation it is a program that starts when the user logs in and keeps
//! running; the menu bar icon comes with the application shell. Until then,
//! this installs the daemon as a launchd agent for the current user, which
//! is the mechanism the shell will use as well.
//!
//! The agent runs the menu bar shell (`genatrix-menubar`, built from
//! `apps/menubar`) when it is beside this binary, and the shell runs
//! `genatrix serve`; without the shell it runs `serve` directly. Either way
//! it uses the data directory it was installed from, comes back if it
//! crashes, stays down when the user quits it from the menu, and writes its
//! log beside the data. Passwords come from the keychain, so nothing secret
//! is in the property list.

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

/// What launchd starts: the shell with the core behind it, or the core
/// alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Program {
    /// `genatrix-menubar --genatrix <core> --data-dir <dir> --port <port>`.
    Shell {
        /// The menu bar executable.
        shell: PathBuf,
        /// The core it starts.
        core: PathBuf,
    },
    /// `genatrix --data-dir <dir> serve --port <port>`.
    CoreOnly {
        /// The core.
        core: PathBuf,
    },
}

impl Program {
    /// The shell when it is beside the core, otherwise the core alone.
    #[must_use]
    pub fn beside(core: PathBuf) -> Self {
        let shell = core.with_file_name("genatrix-menubar");
        if shell.is_file() {
            Self::Shell { shell, core }
        } else {
            Self::CoreOnly { core }
        }
    }

    fn arguments(&self, data_dir: &Path, port: u16) -> Vec<String> {
        let dir = data_dir.display().to_string();
        match self {
            Self::Shell { shell, core } => vec![
                shell.display().to_string(),
                "--genatrix".into(),
                core.display().to_string(),
                "--data-dir".into(),
                dir,
                "--port".into(),
                port.to_string(),
            ],
            Self::CoreOnly { core } => vec![
                core.display().to_string(),
                "--data-dir".into(),
                dir,
                "serve".into(),
                "--port".into(),
                port.to_string(),
            ],
        }
    }
}

/// The property list for one installation.
///
/// `KeepAlive` with `SuccessfulExit` false: a crash brings it back, a quit
/// from the menu, which exits cleanly, does not (design 09: "退出要从菜单栏
/// 图标里点"). It is back at the next login either way.
#[must_use]
pub fn plist(program: &Program, data_dir: &Path, port: u16, log: &Path) -> String {
    let arguments = program
        .arguments(data_dir, port)
        .iter()
        .fold(String::new(), |mut out, a| {
            out.push_str("    <string>");
            out.push_str(&escape(a));
            out.push_str("</string>\n");
            out
        });
    let log = escape(&log.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
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
/// Says what was installed.
pub fn install(data_dir: &Path, port: u16) -> anyhow::Result<(PathBuf, Program)> {
    let core = std::env::current_exe()?.canonicalize()?;
    let program = Program::beside(core);
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
        plist(&program, data_dir, port, &log_dir.join("genatrix.log")),
    )?;
    launchctl(&["bootstrap", &domain(), &path.display().to_string()])?;
    Ok((path, program))
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
    fn without_the_shell_the_core_is_served_directly() {
        let text = plist(
            &Program::CoreOnly {
                core: PathBuf::from("/opt/genatrix/bin/genatrix"),
            },
            Path::new("/Users/someone/Library/Application Support/Genatrix"),
            7717,
            Path::new("/Users/someone/Library/Application Support/Genatrix/logs/genatrix.log"),
        );
        assert!(text.contains("<string>/opt/genatrix/bin/genatrix</string>"));
        assert!(text.contains("<string>--data-dir</string>"));
        assert!(text.contains("<string>serve</string>"));
        assert!(text.contains("<string>7717</string>"));
        assert!(text.contains("<key>SuccessfulExit</key>\n    <false/>"));
        assert!(
            !text.contains("PASSWORD"),
            "nothing secret goes in the plist"
        );
    }

    #[test]
    fn with_the_shell_the_shell_is_started_and_told_where_the_core_is() {
        let text = plist(
            &Program::Shell {
                shell: PathBuf::from("/opt/genatrix/bin/genatrix-menubar"),
                core: PathBuf::from("/opt/genatrix/bin/genatrix"),
            },
            Path::new("/data"),
            7717,
            Path::new("/data/logs/genatrix.log"),
        );
        assert!(text.contains("<string>/opt/genatrix/bin/genatrix-menubar</string>"));
        assert!(text.contains(
            "<string>--genatrix</string>\n    <string>/opt/genatrix/bin/genatrix</string>"
        ));
        assert!(
            !text.contains("<string>serve</string>"),
            "the shell runs serve itself"
        );
    }

    #[test]
    fn the_shell_is_used_only_when_it_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("genatrix");
        std::fs::write(&core, b"").unwrap();
        assert!(matches!(
            Program::beside(core.clone()),
            Program::CoreOnly { .. }
        ));
        std::fs::write(dir.path().join("genatrix-menubar"), b"").unwrap();
        assert!(matches!(Program::beside(core), Program::Shell { .. }));
    }

    #[test]
    fn paths_with_markup_characters_are_escaped() {
        let text = plist(
            &Program::CoreOnly {
                core: PathBuf::from("/tmp/a&b/genatrix"),
            },
            Path::new("/tmp/<data>"),
            1,
            Path::new("/tmp/log"),
        );
        assert!(text.contains("/tmp/a&amp;b/genatrix"));
        assert!(text.contains("/tmp/&lt;data&gt;"));
    }
}
