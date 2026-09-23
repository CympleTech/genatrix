//! The macOS sandbox profile the mail connector runs under.
//!
//! Design: `docs/design/05-connectors.md`, "进程模型"; verified in
//! `docs/plan/spikes/01-sandbox.md` and by a probe during M1.
//!
//! What the operating system enforces here, and what it does not, said
//! plainly. The sandbox filters outbound connections by port, not by host
//! name: it cannot resolve names, and the addresses behind `imap.gmail.com`
//! change. So the profile allows the ports the account capabilities name,
//! 993 for IMAP and the submission ports for SMTP, and nothing else. A
//! subverted connector could reach some other server on port 993, and
//! nothing on any other port. The host check itself is the connector's
//! (`Sync::check_host`), the second of the two layers design 05 describes.
//!
//! Name resolution goes through the system resolver's socket, which has to
//! be allowed by path. The core's own socket is allowed by directory.
//!
//! Files: the connector may not read the data directory, where the
//! database, the raw records and, in a development directory, the master
//! key live; only the `run` directory with the sockets is open to it. It
//! may not read the user's keychain files, and it may not start any other
//! program, so a subverted connector cannot reach the keychain through the
//! `security` tool either. Everything else on the system stays readable,
//! because TLS, name resolution and the dynamic linker need more of it than
//! is worth enumerating.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

/// Where macOS answers DNS questions. Canonical: `/var` is a symlink.
const RESOLVER_SOCKET: &str = "/private/var/run/mDNSResponder";

/// What the profile is built from.
#[derive(Clone, Debug)]
pub struct Confinement<'a> {
    /// Ports the accounts may reach, from their capabilities.
    pub ports: &'a BTreeSet<u16>,
    /// The data directory, which the connector may not read.
    pub data_dir: &'a Path,
    /// The `run` directory inside it, holding the sockets, which it may.
    pub run_dir: &'a Path,
    /// The connector binary, the only program it may become.
    pub binary: &'a Path,
}

/// Render the profile.
pub fn profile(confinement: &Confinement<'_>) -> anyhow::Result<String> {
    if confinement.ports.is_empty() {
        anyhow::bail!("a connector with no hosts to reach has no reason to run");
    }
    std::fs::create_dir_all(confinement.run_dir)?;
    let run_dir = confinement.run_dir.canonicalize()?;
    let data_dir = confinement.data_dir.canonicalize()?;
    let binary = confinement.binary.canonicalize()?;
    let (run_dir, data_dir, binary) = (
        quoted(&run_dir.to_string_lossy()),
        quoted(&data_dir.to_string_lossy()),
        quoted(&binary.to_string_lossy()),
    );
    let keychains = std::env::var_os("HOME")
        .map(|home| Path::new(&home).join("Library/Keychains"))
        .map(|p| quoted(&p.to_string_lossy()));

    let mut out = String::from(
        "(version 1)\n\
         ;; genatrix-imap: outbound only to the mail ports the accounts were granted,\n\
         ;; the system resolver, and the core's socket; no data directory, no keychain\n\
         ;; files, no other programs.\n\
         (allow default)\n\
         (deny network*)\n",
    );
    for port in confinement.ports {
        let _ = writeln!(out, "(allow network-outbound (remote ip \"*:{port}\"))");
    }
    let _ = writeln!(
        out,
        "(allow network-outbound (remote unix-socket (path-literal \"{RESOLVER_SOCKET}\")))"
    );
    let _ = writeln!(
        out,
        "(allow network-outbound (remote unix-socket (subpath {run_dir})))"
    );
    let _ = writeln!(out, "(deny file-read* file-write* (subpath {data_dir}))");
    let _ = writeln!(out, "(allow file-read* file-write* (subpath {run_dir}))");
    if let Some(keychains) = keychains {
        let _ = writeln!(out, "(deny file-read* file-write* (subpath {keychains}))");
    }
    let _ = writeln!(out, "(deny process-exec*)");
    let _ = writeln!(out, "(allow process-exec (literal {binary}))");
    Ok(out)
}

/// A path as a sandbox profile string literal: backslashes and quotes
/// escaped, so a directory name with either cannot end the string and add a
/// rule of its own. The profile language reads C-style escapes.
fn quoted(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for c in path.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gx-imap-sb-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let binary = dir.join("genatrix-imap");
        std::fs::write(&binary, b"").unwrap();
        (dir, binary)
    }

    #[test]
    fn the_profile_opens_exactly_the_granted_ports_and_closes_the_data() {
        let (dir, binary) = setup("ports");
        let ports: BTreeSet<u16> = [993, 587].into_iter().collect();
        let text = profile(&Confinement {
            ports: &ports,
            data_dir: &dir,
            run_dir: &dir.join("run"),
            binary: &binary,
        })
        .unwrap();
        assert!(text.contains("(deny network*)"));
        assert!(text.contains("(remote ip \"*:993\")"));
        assert!(text.contains("(remote ip \"*:587\")"));
        assert!(!text.contains("*:443"));
        assert!(text.contains("mDNSResponder"));
        assert!(
            text.contains("(deny file-read* file-write* (subpath"),
            "{text}"
        );
        assert!(text.contains("Library/Keychains"));
        assert!(text.contains("(deny process-exec*)"));
        assert!(text.contains("(allow process-exec (literal"));
        assert!(
            !text.contains("/var/folders") || text.contains("/private/var/folders"),
            "paths are canonical"
        );
        // The deny of the data directory comes before the allow of run/,
        // because in SBPL the later rule wins.
        let deny_at = text.find("(deny file-read* file-write* (subpath").unwrap();
        let allow_at = text.find("(allow file-read* file-write* (subpath").unwrap();
        assert!(deny_at < allow_at);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_ports_means_no_profile() {
        let (dir, binary) = setup("none");
        assert!(
            profile(&Confinement {
                ports: &BTreeSet::new(),
                data_dir: &dir,
                run_dir: &dir.join("run"),
                binary: &binary,
            })
            .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
