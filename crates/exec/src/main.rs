//! `genatrix-exec`: one executor session, driven for the core.
//!
//! Design: `docs/design/13-executors.md`, "宿主". The core starts this for
//! each step and speaks to it on standard input and output (see
//! [`genatrix_exec::wire`]). The first frame says what to run; after that,
//! a `cancel` frame, or the core closing the pipe, stops the session.

use genatrix_exec::wire::{self, FromHost, ToHost};
use tokio::sync::{mpsc, oneshot};

#[tokio::main]
async fn main() {
    // Exit as soon as the last word is written. Returning would wait for
    // the reader of standard input, which the core keeps open, and the
    // core waits for this process to end: neither would move.
    let code = match serve().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("genatrix-exec: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn serve() -> anyhow::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let start = match wire::read::<_, ToHost>(&mut stdin).await? {
        Some(ToHost::Start(start)) => *start,
        Some(ToHost::Cancel) | None => return Ok(()),
    };

    let (cancel_tx, cancel_rx) = oneshot::channel();
    // Anything after the start, or the core going away, means stop.
    tokio::spawn(async move {
        let _ = wire::read::<_, ToHost>(&mut stdin).await;
        let _ = cancel_tx.send(());
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<FromHost>();
    let session = tokio::spawn(genatrix_exec::session::run(start, tx, cancel_rx));
    while let Some(frame) = rx.recv().await {
        wire::write(&mut stdout, &frame).await?;
    }
    let end = session.await?;
    wire::write(&mut stdout, &FromHost::End(end)).await?;
    tokio::io::AsyncWriteExt::flush(&mut stdout).await?;
    Ok(())
}
