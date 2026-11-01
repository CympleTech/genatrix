//! How the core runs one executor session: through the `genatrix-exec`
//! process, over its pipes (design 13, "宿主").

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;

use genatrix_exec::wire::{self, End, FromHost, Start, ToHost};
use tokio::sync::{mpsc, oneshot};

/// Something that runs one session to its end.
pub trait Host: Send + Sync {
    /// Run `start`; send what happens to `out`; stop when `cancel` fires.
    fn session(
        &self,
        start: Start,
        out: mpsc::UnboundedSender<FromHost>,
        cancel: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = End> + Send>>;
}

/// The real one: the host binary beside the core.
pub struct ProcessHost;

impl Host for ProcessHost {
    fn session(
        &self,
        start: Start,
        out: mpsc::UnboundedSender<FromHost>,
        cancel: oneshot::Receiver<()>,
    ) -> Pin<Box<dyn Future<Output = End> + Send>> {
        Box::pin(async move {
            match run(start, out, cancel).await {
                Ok(end) => end,
                Err(e) => End {
                    stop: "failed".into(),
                    answer: String::new(),
                    error: Some(format!("the executor host failed: {e}")),
                },
            }
        })
    }
}

async fn run(
    start: Start,
    out: mpsc::UnboundedSender<FromHost>,
    cancel: oneshot::Receiver<()>,
) -> anyhow::Result<End> {
    let binary = crate::connectors::binary_for("exec")
        .ok_or_else(|| anyhow::anyhow!("genatrix-exec is not beside this program"))?;
    let mut child = tokio::process::Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        // Nothing of the core's environment: the host needs none of it.
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    wire::write(&mut stdin, &ToHost::Start(Box::new(start))).await?;
    tokio::spawn(async move {
        if cancel.await.is_ok() {
            let _ = wire::write(&mut stdin, &ToHost::Cancel).await;
        }
        // Holding stdin open until here; dropping it also means stop.
        drop(stdin);
    });
    loop {
        match wire::read::<_, FromHost>(&mut stdout).await? {
            Some(FromHost::End(end)) => {
                let _ = child.wait().await;
                return Ok(end);
            }
            Some(frame) => {
                let _ = out.send(frame);
            }
            None => anyhow::bail!("the host ended without saying how the session went"),
        }
    }
}
