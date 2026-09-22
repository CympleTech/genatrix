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

    fn arguments(&self, data_dir: &Path, listen: &Listen) -> Vec<String> {
        let dir = data_dir.display().to_string();
        let mut args = match self {
            Self::Shell { shell, core } => vec![
                shell.display().to_string(),
                "--genatrix".into(),
                core.display().to_string(),
                "--data-dir".into(),
                dir,
            ],
            Self::CoreOnly { core } => vec![
                core.display().to_string(),
                "--data-dir".into(),
                dir,
                "serve".into(),
            ],
        };
        args.extend(["--port".into(), listen.port.to_string()]);
        if !listen.bind.is_loopback() {
            args.extend(["--bind".into(), listen.bind.to_string()]);
        }
        args
    }
}

/// Where the installed core listens (design 06: loopback, or a private
/// network's address for paired devices).
#[derive(Clone, Copy, Debug)]
pub struct Listen {
    /// Address.
    pub bind: std::net::IpAddr,
    /// Port.
    pub port: u16,
}

/// The property list for one installation.
///
/// `KeepAlive` with `SuccessfulExit` false: a crash brings it back, a quit
/// from the menu, which exits cleanly, does not (design 09: "退出要从菜单栏
/// 图标里点"). It is back at the next login either way.
#[must_use]
pub fn plist(program: &Program, data_dir: &Path, listen: &Listen, log: &Path) -> String {
    let arguments = program
        .arguments(data_dir, listen)
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
pub fn install(data_dir: &Path, listen: &Listen) -> anyhow::Result<(PathBuf, Program)> {
    let core = std::env::current_exe()?.canonicalize()?;
    let program = Program::beside(core);
    let log_dir = data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Out with the old first, quietly: there may be none. launchd tears a
    // job down asynchronously, and a load that lands during the teardown
    // is dropped, so give it a moment.
    if launchctl(&["bootout", &format!("{}/{LABEL}", domain())]).is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    std::fs::write(
        &path,
        plist(&program, data_dir, listen, &log_dir.join("genatrix.log")),
    )?;
    // `bootstrap` needs the caller to be inside the user's GUI session; from
    // SSH or a tool it fails with an I/O error. The older `load` reaches the
    // session from anywhere, so it is the fallback rather than the error.
    if let Err(bootstrap) = launchctl(&["bootstrap", &domain(), &path.display().to_string()]) {
        launchctl(&["load", "-w", &path.display().to_string()])
            .map_err(|load| anyhow::anyhow!("{bootstrap}; then {load}"))?;
    }
    // `load` straight after `bootout` of the same label is sometimes a
    // no-op while launchd is still tearing the old one down. Look, and ask
    // once more if it is not there.
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if running() {
            return Ok((path, program));
        }
        let _ = launchctl(&["load", "-w", &path.display().to_string()]);
    }
    if running() {
        Ok((path, program))
    } else {
        anyhow::bail!(
            "the agent was written to {} but launchd did not start it; \
             `launchctl load -w` that file from a terminal on the machine",
            path.display()
        )
    }
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

/// Whether launchd has the agent and its process is up.
fn running() -> bool {
    Command::new("/bin/launchctl")
        .args(["print", &format!("{}/{LABEL}", domain())])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("state = running"))
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

    const LOOPBACK: Listen = Listen {
        bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        port: 7717,
    };

    #[test]
    fn loopback_is_left_unsaid_and_another_address_is_passed_on() {
        let core = Program::CoreOnly {
            core: PathBuf::from("/opt/genatrix/bin/genatrix"),
        };
        let local = plist(&core, Path::new("/data"), &LOOPBACK, Path::new("/data/log"));
        assert!(!local.contains("--bind"), "the default needs no flag");
        let tailnet = plist(
            &core,
            Path::new("/data"),
            &Listen {
                bind: "100.101.102.103".parse().unwrap(),
                port: 7717,
            },
            Path::new("/data/log"),
        );
        assert!(tailnet.contains("<string>--bind</string>\n    <string>100.101.102.103</string>"));
    }

    #[test]
    fn without_the_shell_the_core_is_served_directly() {
        let text = plist(
            &Program::CoreOnly {
                core: PathBuf::from("/opt/genatrix/bin/genatrix"),
            },
            Path::new("/Users/someone/Library/Application Support/Genatrix"),
            &LOOPBACK,
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
            &LOOPBACK,
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
            &Listen {
                bind: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 1,
            },
            Path::new("/tmp/log"),
        );
        assert!(text.contains("/tmp/a&amp;b/genatrix"));
        assert!(text.contains("/tmp/&lt;data&gt;"));
    }
}
