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

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

/// Where macOS answers DNS questions. Canonical: `/var` is a symlink.
const RESOLVER_SOCKET: &str = "/private/var/run/mDNSResponder";

/// Render the profile for a set of allowed ports and the directory holding
/// the core's socket.
pub fn profile(ports: &BTreeSet<u16>, run_dir: &Path) -> anyhow::Result<String> {
    if ports.is_empty() {
        anyhow::bail!("a connector with no hosts to reach has no reason to run");
    }
    std::fs::create_dir_all(run_dir)?;
    let run_dir = run_dir.canonicalize()?;
    let run_dir = run_dir.to_string_lossy();
    let mut out = String::from(
        "(version 1)\n\
         ;; genatrix-imap: outbound only to the mail ports the accounts were granted,\n\
         ;; the system resolver, and the core's socket.\n\
         (allow default)\n\
         (deny network*)\n",
    );
    for port in ports {
        let _ = writeln!(out, "(allow network-outbound (remote ip \"*:{port}\"))");
    }
    let _ = writeln!(
        out,
        "(allow network-outbound (remote unix-socket (path-literal \"{RESOLVER_SOCKET}\")))"
    );
    let _ = writeln!(
        out,
        "(allow network-outbound (remote unix-socket (subpath \"{run_dir}\")))"
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_opens_exactly_the_granted_ports() {
        let dir = std::env::temp_dir().join(format!("gx-imap-sb-{}", std::process::id()));
        let ports: BTreeSet<u16> = [993, 587].into_iter().collect();
        let text = profile(&ports, &dir).unwrap();
        assert!(text.contains("(deny network*)"));
        assert!(text.contains("(remote ip \"*:993\")"));
        assert!(text.contains("(remote ip \"*:587\")"));
        assert!(!text.contains("*:443"));
        assert!(text.contains("mDNSResponder"));
        assert!(
            !text.contains("/var/folders") || text.contains("/private/var/folders"),
            "paths are canonical"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_ports_means_no_profile() {
        assert!(profile(&BTreeSet::new(), Path::new("/tmp")).is_err());
    }
}
