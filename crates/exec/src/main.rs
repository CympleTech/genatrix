//! `genatrix-exec`: one executor session, driven for the core.
//!
//! Design: `docs/design/13-executors.md`, "宿主". The core starts this for
//! each step and speaks to it on standard input and output (see
//! [`genatrix_exec::wire`]). The first frame says what to run; after that,
//! a `cancel` frame, the core closing the pipe, or a signal to stop, stops
//! the session; the executor and everything it started are ended with it.

use genatrix_exec::wire::{self, FromHost, ToHost};
use tokio::signal::unix::{SignalKind, signal};
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
    // Anything after the start, the core going away (its end of the pipe
    // closes), or being told to stop by a signal, means stop: the session
    // then ends the executor and everything it started (design 13 v0.10).
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        tokio::select! {
            _ = wire::read::<_, ToHost>(&mut stdin) => {}
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
        let _ = cancel_tx.send(());
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<FromHost>();
    let session = tokio::spawn(genatrix_exec::session::run(start, tx, cancel_rx));
    // Once the core cannot be written to, it is gone; the session still
    // runs to its end, so that what it started is ended with it.
    let mut core_there = true;
    while let Some(frame) = rx.recv().await {
        if core_there && wire::write(&mut stdout, &frame).await.is_err() {
            core_there = false;
        }
    }
    let end = session.await?;
    if core_there {
        wire::write(&mut stdout, &FromHost::End(end)).await?;
        tokio::io::AsyncWriteExt::flush(&mut stdout).await?;
    }
    Ok(())
}
