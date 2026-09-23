//! The macOS sandbox profile this process is meant to run under.
//!
//! Everything is allowed except the network, which is then reopened only for
//! the directory holding our own socket. Paths must be canonical: `/var` is a
//! symlink to `/private/var` and the sandbox matches the real path
//! (spike 01). Tightening file access to the model and socket directories is
//! planned once the daemon owns the launch.

use std::path::Path;

/// Render the profile for a socket and a model directory.
pub fn profile(socket: &Path, model_dir: &Path) -> anyhow::Result<String> {
    let sock_dir = socket
        .parent()
        .ok_or_else(|| anyhow::anyhow!("socket path has no parent directory"))?;
    std::fs::create_dir_all(sock_dir)?;
    let sock_dir = sock_dir.canonicalize()?;
    let model_dir = model_dir.canonicalize()?;
    let sock_dir = quoted(&sock_dir.to_string_lossy());
    // A comment runs to the end of the line; a newline in the name must not.
    let model_dir = model_dir.to_string_lossy().replace(['\n', '\r'], " ");
    Ok(format!(
        "(version 1)\n\
         ;; genatrix-infer: no network, except our own Unix socket.\n\
         ;; model: {model_dir}\n\
         (allow default)\n\
         (deny network*)\n\
         (allow network-bind network-inbound network-outbound (subpath {sock_dir}))\n"
    ))
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

    #[test]
    fn profile_denies_network_and_reopens_socket_dir() {
        let dir = std::env::temp_dir().join(format!("gx-infer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = profile(&dir.join("run").join("infer.sock"), &dir).unwrap();
        assert!(p.contains("(deny network*)"));
        assert!(p.contains("network-bind network-inbound network-outbound (subpath"));
        assert!(!p.contains("/var/folders") || p.contains("/private/var/folders"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hostile_directory_name_stays_inside_its_string_and_its_comment() {
        let base = std::env::temp_dir().join(format!("gx-infer-q-{}", std::process::id()));
        let odd = base.join("a \" b\\c\n(allow network*)");
        std::fs::create_dir_all(&odd).unwrap();
        let p = profile(&odd.join("run").join("infer.sock"), &odd).unwrap();
        // The name's text may appear, but only inside the comment line and
        // inside the escaped string: never as a rule of its own.
        assert!(
            !p.lines()
                .any(|l| l.trim_start().starts_with("(allow network*)")),
            "{p}"
        );
        assert_eq!(
            p.lines().filter(|l| l.starts_with(";;")).count(),
            2,
            "one comment line each"
        );
        assert!(p.contains("\\\" b\\\\c\\n"), "{p}");
        assert_eq!(
            p.lines()
                .filter(|l| l.starts_with("(allow network"))
                .count(),
            1,
            "{p}"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
