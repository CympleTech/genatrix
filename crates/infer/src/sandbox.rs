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
    let sock_dir = sock_dir.to_string_lossy();
    let model_dir = model_dir.to_string_lossy();
    Ok(format!(
        "(version 1)\n\
         ;; genatrix-infer: no network, except our own Unix socket.\n\
         ;; model: {model_dir}\n\
         (allow default)\n\
         (deny network*)\n\
         (allow network-bind network-inbound network-outbound (subpath \"{sock_dir}\"))\n"
    ))
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
}
