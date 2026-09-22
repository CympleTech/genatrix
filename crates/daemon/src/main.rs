//! `genatrix`: the core process.
//!
//! Design: `docs/design/06-interface.md` and
//! `docs/design/09-install-recover-migrate.md`.
//!
//! Not yet a daemon: it has no web interface and no connectors, so what it
//! offers is the commands needed to see the whole path work end to end. Each
//! one is a step the finished product will take on its own.

#![forbid(unsafe_code)]

mod accounts;
mod actions;
mod caller;
mod config;
mod connectors;
mod conversation;
mod ingest;
mod keychain;
mod keys;
mod models;
mod names;
mod pipeline;
mod seed;
mod service;
mod syncing;
mod system;
mod web;

use std::path::PathBuf;

use chrono::{Timelike, Utc};

use clap::{Parser, Subcommand};
use genatrix_agent::run::RunContext;
use genatrix_keys::TicketKey;
use genatrix_ledger::{EntryFilter, kind};
use genatrix_store::{ItemQuery, ItemVersion};

use crate::config::Config;
use crate::system::System;

/// Environment variable carrying the key shared with the gateway.
const KEY_ENV: &str = "GENATRIX_TICKET_KEY";

#[derive(Debug, Parser)]
#[command(name = "genatrix", about = "Genatrix core")]
struct Args {
    /// Data directory. Defaults to ~/Library/Application Support/Genatrix.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create the data directory, the keys, and the databases.
    Init,
    /// Put synthetic items in, so there is something to work on before a
    /// connector exists.
    Seed,
    /// Judge the sensitivity of every item that has none yet.
    Classify,
    /// Read every raw record again with today's normalization. An item
    /// whose text or payload changes gets a new version; nothing is fetched.
    Reprocess,
    /// Make today's digest now, from the last 24 hours, and print it.
    Digest,
    /// Show the timeline.
    Timeline {
        /// How many items.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show what has left this device.
    Ledger,
    /// Add a mailbox, or sign in to one again. Asks for the app password,
    /// checks it against the server, and keeps it in the keychain.
    Account {
        /// The address. For a well-known provider that is all it takes.
        #[arg(long, required_unless_present_any = ["forget", "add_telegram"])]
        add: Option<String>,
        /// The IMAP server, when it cannot be worked out from the address.
        #[arg(long)]
        imap_host: Option<String>,
        /// Remove a mailbox and its password. What was fetched stays.
        #[arg(long, conflicts_with = "add")]
        forget: Option<String>,
        /// Sign in to Telegram with this phone number (international form,
        /// `+64...`). Asks for the code Telegram sends, and the password if
        /// two-step verification is on.
        #[arg(long, conflicts_with_all = ["add", "forget"], required_unless_present_any = ["add", "forget"])]
        add_telegram: Option<String>,
    },
    /// Run in the background from login onwards, or stop doing so.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Fetch mail for every account.
    Sync {
        /// Stop after this many messages per account.
        #[arg(long, default_value_t = 500)]
        limit: usize,
    },
    /// Write everything out in the open format, so it can be read elsewhere.
    Export {
        /// Directory to write into. Created if it does not exist.
        #[arg(long)]
        to: PathBuf,
    },
    /// Serve the interface.
    Serve {
        /// Address to listen on. Loopback by default, which only this machine
        /// can reach. A private network's address, such as a Tailscale or
        /// other wireguard tunnel, lets paired devices reach it; pair them
        /// from Settings on this machine.
        #[arg(long, default_value = "127.0.0.1")]
        bind: std::net::IpAddr,
        /// Port.
        #[arg(long, default_value_t = 7717)]
        port: u16,
    },
}

#[derive(Debug, Subcommand)]
enum ServiceAction {
    /// Install a launch agent that runs Genatrix for this data directory
    /// from login onwards: the menu bar shell when it is beside this binary,
    /// otherwise `serve` alone.
    Install {
        /// Port for the interface.
        #[arg(long, default_value_t = 7717)]
        port: u16,
        /// Address for the interface. Loopback by default; a private
        /// network's address lets paired devices reach it.
        #[arg(long, default_value = "127.0.0.1")]
        bind: std::net::IpAddr,
    },
    /// Stop it and remove the launch agent.
    Uninstall,
    /// Say whether it is installed.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,genatrix=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let config = Config::under(args.data_dir.unwrap_or_else(Config::default_data_dir));

    match args.command {
        Command::Init => init(&config),
        Command::Seed => seed(&config),
        Command::Classify => classify(&config).await,
        Command::Reprocess => reprocess(&config),
        Command::Digest => digest_now(&config).await,
        Command::Timeline { limit } => timeline(&config, limit),
        Command::Ledger => ledger(&config),
        Command::Export { to } => export(&config, &to),
        Command::Account {
            add: Some(address),
            imap_host,
            ..
        } => account(&config, &address, imap_host).await,
        Command::Account {
            forget: Some(address),
            ..
        } => forget_account(&config, &address),
        Command::Account {
            add_telegram: Some(phone),
            ..
        } => add_telegram(&config, &phone).await,
        Command::Account { .. } => {
            anyhow::bail!("say which account: --add, --forget or --add-telegram")
        }
        Command::Service { action } => service(&config, &action),
        Command::Sync { limit } => sync(&config, limit).await,
        Command::Serve { bind, port } => serve(&config, &web::Serving { bind, port }).await,
    }
}

/// A key is needed for anything that might call a model. Commands that only
/// read use a throwaway one: nothing they do can mint a usable ticket.
/// The ticket key: from the environment, else the one the running daemon
/// left in `run/ticket.key`, else a fresh one, or an error when a command
/// cannot do without the gateway's.
fn ticket_key(config: &Config, required: bool) -> anyhow::Result<TicketKey> {
    if let Ok(hex) = std::env::var(KEY_ENV) {
        return Ok(TicketKey::from_hex(&hex)?);
    }
    let left_behind = config.ticket_key_path();
    if let Ok(hex) = std::fs::read_to_string(&left_behind) {
        return Ok(TicketKey::from_hex(hex.trim())?);
    }
    if !required {
        return Ok(TicketKey::generate()?);
    }
    anyhow::bail!(
        "{KEY_ENV} is not set and {} does not exist. Either `genatrix serve` is running, \
         which leaves its key there, or set the same value the gateway was started with:\n\n  \
         export {KEY_ENV}=<the gateway's key>\n\n\
         It is the secret that lets this process mint permission to send.",
        left_behind.display()
    )
}

/// Leave the key where the other commands of this user can find it while
/// the daemon runs. Readable by this user only, like the sockets beside it.
fn leave_ticket_key(config: &Config, key: &TicketKey) -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;

    let path = config.ticket_key_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::File::create(&path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    writeln!(file, "{}", key.to_hex())?;
    Ok(())
}

fn init(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    println!("data directory  {}", config.data_dir.display());
    println!("store           {} item(s)", system.store.count_items()?);
    println!(
        "files           {} bytes of raw records, {} bytes of attachments",
        system.raw_files.size_on_disk()?,
        system.blob_files.size_on_disk()?
    );
    println!("ledger          {} entr(ies)", system.ledger.len()?);
    println!("rules           {}", config.rules_path().display());
    println!("gateway socket  {}", config.gateway_socket.display());
    println!(
        "cloud           {}",
        if system.gate.cloud_enabled() {
            "enabled"
        } else {
            "off"
        }
    );
    Ok(())
}

fn seed(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    let added = seed::run(&system.store, &system.raw_files)?;
    println!(
        "added {added} of {} sample item(s); {} in the store, {} bytes of raw records on disk",
        seed::count(),
        system.store.count_items()?,
        system.raw_files.size_on_disk()?
    );
    Ok(())
}

async fn classify(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, true)?)?;
    if !system.caller.gateway_healthy().await {
        anyhow::bail!(
            "the gateway is not answering at {}. Start it, and the inference \
             process behind it, before classifying.",
            config.gateway_socket.display()
        );
    }

    let mut ctx = RunContext::begin(&system.ledger, &system.caller, "classify", 64)?;
    let store = system.store.clone();
    let started = std::time::Instant::now();
    let report = pipeline::classify::run(&store, system.gate.rules(), &mut ctx).await;
    let steps = ctx.steps();
    let report = match report {
        Ok(report) => {
            ctx.done()?;
            report
        }
        Err(genatrix_agent::run::RunError::OutOfSteps { max }) => {
            ctx.stopped("paused at the step budget")?;
            println!(
                "paused after {max} model call(s), the budget of one run; \
                 run `genatrix classify` again to continue where this left off."
            );
            return Ok(());
        }
        Err(e) => {
            ctx.stopped(e.to_string())?;
            return Err(e.into());
        }
    };

    println!(
        "classified {} item(s) in {:.1?}",
        report.seen,
        started.elapsed()
    );
    println!("  settled by rules alone   {}", report.by_rule);
    println!(
        "  asked the local model    {} in {steps} call(s)",
        report.asked
    );
    println!("  raised above the rules   {}", report.raised);
    for (level, count) in &report.levels {
        println!("  {level:<24} {count}");
    }
    Ok(())
}

/// Environment variable carrying a mailbox password, read at sync time and
/// never written down. Design 08 replaces this with the keychain.
const PASSWORD_ENV: &str = "GENATRIX_IMAP_PASSWORD";

/// Derive every item again from its raw record.
fn reprocess(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    let raws = system.store.all_raw()?;
    let (mut unchanged, mut updated, mut unreadable) = (0usize, 0usize, 0usize);
    for raw in &raws {
        match ingest::reprocess(&system.store, &system.raw_files, &system.blob_files, raw)? {
            ingest::Reprocessed::Unchanged => unchanged += 1,
            ingest::Reprocessed::Updated(_) => updated += 1,
            ingest::Reprocessed::Unreadable(why) => {
                unreadable += 1;
                tracing::warn!(source = %raw.source.external_id, %why, "raw record no longer parses");
            }
        }
    }
    let telegram_ids: Vec<i64> = accounts::Accounts::load(&config.accounts_path())?
        .telegram
        .values()
        .map(|a| a.user_id)
        .collect();
    let repaired = ingest::repair_chats(&system.store, &telegram_ids)?;
    println!("{} raw record(s) read again", raws.len());
    if repaired > 0 {
        println!("  {repaired} chat message(s) given their direction and recipient");
    }
    println!("  {updated} item(s) brought up to date");
    println!("  {unchanged} unchanged");
    if unreadable > 0 {
        println!("  {unreadable} could not be parsed; their items stay as they were");
    }
    if updated > 0 {
        println!("Their sensitivity was kept; `genatrix classify --again` is not a thing yet.");
    }
    Ok(())
}

/// Where a mail password comes from: the environment for development,
/// otherwise the keychain. Nothing else, and never a file.
fn password_for(address: &str) -> anyhow::Result<Option<String>> {
    if let Ok(password) = std::env::var(PASSWORD_ENV) {
        return Ok(Some(password));
    }
    keychain::Keychain::mail().read(address)
}

/// Add a mailbox, or sign in to one again (design 05, "接入").
///
/// The password is asked for, tried against the server, and only then kept
/// in the keychain. A refused password is not stored, because storing it
/// would just move the same failure to the background where nobody sees
/// it. The account's address becomes a handle of the user's own person, so
/// direction is known from the first message.
async fn account(config: &Config, address: &str, imap_host: Option<String>) -> anyhow::Result<()> {
    use std::io::Write as _;

    use genatrix_connector::capability::Host;
    use genatrix_connector_imap::{Credentials, Imap};
    use genatrix_model::HandleKind;

    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    let path = config.accounts_path();
    let mut accounts = accounts::Accounts::load(&path)?;
    accounts.add_mail(address, imap_host)?;
    let account = accounts.mail[&address.trim().to_lowercase()].clone();

    println!("{}", account.address);
    println!("  reads from  {}:{}", account.imap_host, account.imap_port);
    match &account.smtp_host {
        Some(host) => println!("  sends via   {host}:{}", account.smtp_port),
        None => println!("  sends via   not configured, so this account cannot send"),
    }

    let password = if let Ok(p) = std::env::var(PASSWORD_ENV) {
        p
    } else {
        println!();
        println!("On Gmail this is an app password, not your account password:");
        println!("turn on two-step verification, then create one under App passwords.");
        rpassword::prompt_password(format!("App password for {}: ", account.address))?
    };
    let password = password.trim().to_owned();
    if password.is_empty() {
        anyhow::bail!("no password given; nothing was changed");
    }

    print!("  checking    ");
    std::io::stdout().flush()?;
    let credentials = Credentials {
        account: account.address.clone(),
        password: password.clone(),
        imap: Host::new(&account.imap_host, account.imap_port),
    };
    match Imap::connect(&credentials).await {
        Ok(imap) => {
            let _ = imap.logout().await;
            println!("signed in");
        }
        Err(fault) => {
            println!("failed");
            anyhow::bail!("{}\nThe password was not stored.", fault.detail);
        }
    }

    keychain::Keychain::mail().store(&account.address, &password)?;
    println!("  password    in the keychain");
    accounts.save(&path)?;

    // The address is the user's own.
    let me = system
        .store
        .person_for_handle(HandleKind::Email, &account.address, "")?;
    if system.store.self_person()?.is_none() {
        system.store.set_self(me)?;
    }

    println!();
    println!("`genatrix serve` keeps this mailbox up to date while it runs;");
    println!("`genatrix service install` makes that happen from login onwards.");
    Ok(())
}

/// Sign in to Telegram (design 05, "接入"; design 09, screen four): the
/// number, the code, the password if asked. The session is kept in the
/// encrypted store, under the account's cursors; the account's own user id
/// becomes a handle of the user's person.
async fn add_telegram(config: &Config, phone: &str) -> anyhow::Result<()> {
    use genatrix_connector_telegram::login::{Prompt, sign_in};
    use genatrix_model::HandleKind;

    struct Terminal;
    impl Prompt for Terminal {
        fn code(&self) -> anyhow::Result<String> {
            use std::io::Write as _;
            print!("Telegram sent a code to your phone or another signed-in device. Code: ");
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            Ok(line)
        }
        fn password(&self, hint: Option<&str>) -> anyhow::Result<String> {
            let hint = hint
                .filter(|h| !h.is_empty())
                .map_or(String::new(), |h| format!(" (hint: {h})"));
            Ok(rpassword::prompt_password(format!(
                "Two-step verification password{hint}: "
            ))?)
        }
    }

    let phone = phone.trim();
    if !phone.starts_with('+') {
        anyhow::bail!("the number needs its country code, as in +64 21 ...");
    }
    let credentials = genatrix_connector_telegram::Credentials::find()?;
    let system = System::open(config.clone(), ticket_key(config, false)?)?;

    println!("{phone}");
    println!("  Telegram will show a new device signed in; that is this.");
    let signed_in = sign_in(&credentials, phone, &Terminal).await?;
    println!("  signed in as {} ({})", signed_in.name, signed_in.user_id);

    system.store.put_sync_cursor(
        genatrix_model::Connector::Telegram,
        phone,
        "session",
        &serde_json::to_string(&signed_in.session)?,
    )?;
    let path = config.accounts_path();
    let mut accounts = accounts::Accounts::load(&path)?;
    accounts.add_telegram(phone, signed_in.user_id, &signed_in.name);
    accounts.save(&path)?;

    // The address is the user's own (design 01: every account's handle
    // hangs on the self person). With a self person already there from an
    // earlier account, the handle joins it rather than making a stranger.
    if let Some(me) = system.store.self_person()? {
        let value = signed_in.user_id.to_string();
        if let Some(existing) = system.store.find_handle(HandleKind::TelegramId, &value)? {
            if existing.person_id != me.id {
                system
                    .store
                    .move_handle(HandleKind::TelegramId, &value, me.id)?;
                system.store.reassign_author(existing.person_id, me.id)?;
            }
        } else {
            system.store.insert_handle(&genatrix_model::Handle {
                id: genatrix_model::HandleId::new(),
                person_id: me.id,
                kind: HandleKind::TelegramId,
                value,
                confidence: genatrix_model::Confidence::Confirmed,
            })?;
        }
    } else {
        let me = system.store.person_for_handle(
            HandleKind::TelegramId,
            &signed_in.user_id.to_string(),
            &signed_in.name,
        )?;
        system.store.set_self(me)?;
    }
    println!("  session     in the encrypted store");
    println!();
    println!(
        "`genatrix serve` reads this account while it runs; restart the service to pick it up."
    );
    Ok(())
}

/// Remove a mailbox from the list and its password from the keychain. Items
/// already fetched stay, as design 05 says for a stopped account.
fn forget_account(config: &Config, address: &str) -> anyhow::Result<()> {
    let path = config.accounts_path();
    let mut accounts = accounts::Accounts::load(&path)?;
    let address = address.trim().to_lowercase();
    let listed = accounts.mail.remove(&address).is_some();
    accounts.save(&path)?;
    let had_password = keychain::Keychain::mail().forget(&address)?;
    match (listed, had_password) {
        (false, false) => println!("{address} was not known"),
        _ => println!("{address} removed; what it fetched stays"),
    }
    Ok(())
}

fn service(config: &Config, action: &ServiceAction) -> anyhow::Result<()> {
    match action {
        ServiceAction::Install { port, bind } => {
            if !config.gateway_config_path().exists() {
                anyhow::bail!(
                    "nothing to install yet: {} has not been initialised",
                    config.data_dir.display()
                );
            }
            let listen = service::Listen {
                bind: *bind,
                port: *port,
            };
            let (path, program) = service::install(&config.data_dir, &listen)?;
            println!("installed  {}", path.display());
            match program {
                service::Program::Shell { .. } => {
                    println!("runs       the menu bar shell, with the core behind it");
                }
                service::Program::CoreOnly { .. } => {
                    println!("runs       the core alone; build apps/menubar for the menu bar icon");
                }
            }
            println!("serving    http://127.0.0.1:{port}");
            println!(
                "log        {}",
                config.data_dir.join("logs/genatrix.log").display()
            );
            println!("It starts now and at every login. `genatrix service uninstall` stops it.");
        }
        ServiceAction::Uninstall => {
            if service::uninstall()? {
                println!("stopped and removed");
            } else {
                println!("it was not installed");
            }
        }
        ServiceAction::Status => {
            if service::installed() {
                println!("installed and known to launchd: {}", service::LABEL);
            } else {
                println!("not installed");
            }
        }
    }
    Ok(())
}

/// Fetch mail for every account, one round at a time, until the history is
/// in or the limit is reached. Cursors are kept, so running it again
/// continues rather than starting over. `genatrix serve` does the same
/// continuously.
async fn sync(config: &Config, limit: usize) -> anyhow::Result<()> {
    use genatrix_connector::SyncState;
    use genatrix_connector::capability::Host;
    use genatrix_connector_imap::sync::Sync as MailSync;
    use genatrix_connector_imap::{Credentials, Imap, Watcher};

    let system = std::sync::Arc::new(System::open(config.clone(), ticket_key(config, false)?)?);
    let accounts = accounts::Accounts::load(&config.accounts_path())?;
    if accounts.mail.is_empty() {
        anyhow::bail!("no accounts yet. `genatrix account --add you@example.com` adds one.");
    }
    let grant = accounts.grant();
    let started = std::time::Instant::now();
    let mut total = 0usize;

    for account in accounts.mail.values() {
        let Some(password) = password_for(&account.address)? else {
            anyhow::bail!(
                "no password for {}. `genatrix account --add {}` asks for one and keeps it \
                 in the keychain.",
                account.address,
                account.address
            );
        };
        let host = Host::new(&account.imap_host, account.imap_port);
        let capability = grant
            .account(&account.address)
            .ok_or_else(|| anyhow::anyhow!("{} has no capability", account.address))?
            .clone();
        if !capability.may_reach(&host) {
            anyhow::bail!("{} is not allowed to connect to {host}", account.address);
        }

        println!("{}", account.address);
        let credentials = Credentials {
            account: account.address.clone(),
            password,
            imap: host,
        };
        let imap = Imap::connect(&credentials).await?;
        let (status, _) = tokio::sync::watch::channel(SyncState::Starting);
        let sink = syncing::StoreSink::new(system.clone(), &account.address);
        let mut watcher = Watcher::new(MailSync::new(imap, capability), sink, status);

        let mut taken = 0usize;
        loop {
            let round = watcher.round().await?;
            taken += round.new;
            if round.complete {
                println!("    {}", watcher.progress().describe());
                break;
            }
            println!("    {}", watcher.progress().describe());
            if taken >= limit {
                println!("  stopping at {limit}; running `genatrix sync` again carries on");
                break;
            }
        }
        println!("  {taken} new item(s)");
        total += taken;
    }

    let elapsed = started.elapsed();
    println!();
    println!(
        "{total} new item(s) in {}, {} per minute",
        describe_duration(elapsed),
        rate_per_minute(total, elapsed)
    );
    println!("{} item(s) in the store", system.store.count_items()?);
    println!("`genatrix classify` judges the new ones.");
    Ok(())
}

/// `1m 23s`, or `4s` when it was quick.
fn describe_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Whole items per minute.
fn rate_per_minute(count: usize, d: std::time::Duration) -> u64 {
    let millis = d.as_millis().max(1);
    (count as u128 * 60_000 / millis)
        .try_into()
        .unwrap_or(u64::MAX)
}

/// The ingestion pipelines, run as mail arrives and while the model side
/// is up: judge what is new, vectorize what has no vectors, summarize what
/// has no summary (design 03). Each pass has its own run and step budget;
/// a pass that stops at the budget continues on the next tick, and what it
/// did stays done.
async fn pipelines_as_mail_arrives(system: std::sync::Arc<System>) {
    let mut judged_at_count: Option<u64> = None;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        if !system.model_state.borrow().is_ready() {
            continue;
        }
        let count = match system.store.count_items() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "could not count items");
                continue;
            }
        };

        if judged_at_count != Some(count)
            && let Some(mut ctx) = begin_run(&system, "classify")
        {
            let result = pipeline::classify::run(&system.store, system.gate.rules(), &mut ctx)
                .await
                .map(|r| {
                    (r.seen > 0).then(|| {
                        format!(
                            "classified {} item(s): {} by rules, {} asked, {} raised",
                            r.seen, r.by_rule, r.asked, r.raised
                        )
                    })
                });
            if finish_run(ctx, "classify", result) {
                judged_at_count = Some(count);
            }
        }

        if system.embedder_configured()
            && let Some(mut ctx) = begin_run(&system, "embed")
        {
            let result = pipeline::embed::run(&system.store, &mut ctx)
                .await
                .map(|r| {
                    (r.items > 0)
                        .then(|| format!("embedded {} item(s) in {} chunk(s)", r.items, r.chunks))
                });
            finish_run(ctx, "embed", result);
        }

        if let Some(mut ctx) = begin_run(&system, "summarize") {
            let result = pipeline::summarize::run(&system.store, &mut ctx)
                .await
                .map(|r| {
                    (r.summarized > 0).then(|| format!("summarized {} item(s)", r.summarized))
                });
            finish_run(ctx, "summarize", result);
        }

        match system.actions.expire_due(&system.store, &system.ledger) {
            Ok(0) => {}
            Ok(n) => tracing::info!(n, "action(s) expired"),
            Err(e) => tracing::warn!(error = %e, "could not expire actions"),
        }

        if let Some(mut ctx) = begin_run(&system, "commitments") {
            let result = pipeline::commitments::run(&system.store, &mut ctx)
                .await
                .map(|r| {
                    (r.found > 0).then(|| {
                        format!("read {} message(s), found {} promise(s)", r.read, r.found)
                    })
                });
            finish_run(ctx, "commitments", result);
        }

        // The morning digest (design 09: eight o'clock by default). Once a
        // day, after the hour, when there is none for today yet; a machine
        // asleep at eight makes it a few minutes after waking.
        let now = chrono::Local::now();
        if now.hour() >= DIGEST_HOUR
            && matches!(system.store.get_digest(now.date_naive()), Ok(None))
            && let Some(mut ctx) = begin_run(&system, "daily-digest")
        {
            let result = pipeline::digest::run(
                &system.store,
                &mut ctx,
                Utc::now(),
                now.date_naive(),
            )
            .await
            .and_then(|digest| {
                system.store.put_digest(&digest).map_err(|e| {
                    genatrix_agent::run::RunError::Ledger(pipeline::store_error(&e))
                })?;
                Ok(Some(format!(
                    "digest for {}: {} to reply, {} promised, {} worth knowing, from {} item(s)",
                    digest.day,
                    digest.points(genatrix_model::DigestGroup::NeedsReply).len(),
                    digest.points(genatrix_model::DigestGroup::Promised).len(),
                    digest
                        .points(genatrix_model::DigestGroup::WorthKnowing)
                        .len(),
                    digest.considered
                )))
            });
            finish_run(ctx, "daily-digest", result);
        }
    }
}

/// When the digest is made, local time (design 09: default eight).
const DIGEST_HOUR: u32 = 8;

/// Make today's digest now and print it.
async fn digest_now(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, true)?)?;
    if !system.caller.gateway_healthy().await {
        anyhow::bail!("the gateway is not answering; is `genatrix serve` running?");
    }
    let now = chrono::Local::now();
    let mut ctx = RunContext::begin(&system.ledger, &system.caller, "daily-digest", 64)?;
    let started = std::time::Instant::now();
    let digest =
        match pipeline::digest::run(&system.store, &mut ctx, Utc::now(), now.date_naive()).await {
            Ok(d) => {
                ctx.done()?;
                d
            }
            Err(e) => {
                ctx.stopped(e.to_string())?;
                return Err(e.into());
            }
        };
    system.store.put_digest(&digest)?;
    println!(
        "digest for {} from {} item(s), in {:.0?}",
        digest.day,
        digest.considered,
        started.elapsed()
    );
    for (group, points) in &digest.groups {
        let name = match group {
            genatrix_model::DigestGroup::NeedsReply => "Needs your reply",
            genatrix_model::DigestGroup::Promised => "You promised",
            genatrix_model::DigestGroup::WorthKnowing => "Worth knowing",
        };
        println!("\n{name} ({})", points.len());
        for p in points {
            println!("  · {}", p.text);
        }
    }
    Ok(())
}

type Run<'a> = RunContext<'a, caller::GatewayCaller<names::StoreNames>>;

fn begin_run<'a>(system: &'a System, task: &str) -> Option<Run<'a>> {
    match RunContext::begin(&system.ledger, &system.caller, task, 64) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            tracing::warn!(task, error = %e, "could not begin a run");
            None
        }
    }
}

/// Close a run. Returns whether the pass ran to its end; stopping at the
/// step budget is not a failure, just not the end.
fn finish_run(
    ctx: Run<'_>,
    task: &str,
    result: Result<Option<String>, genatrix_agent::run::RunError>,
) -> bool {
    match result {
        Ok(line) => {
            let _ = ctx.done();
            if let Some(line) = line {
                tracing::info!(task, "{line}");
            }
            true
        }
        Err(genatrix_agent::run::RunError::OutOfSteps { .. }) => {
            let _ = ctx.stopped("paused at the step budget; more next pass");
            tracing::info!(task, "paused at the step budget; continuing next pass");
            false
        }
        Err(e) => {
            let _ = ctx.stopped(e.to_string());
            tracing::warn!(task, error = %e, "pass failed; will try again");
            false
        }
    }
}

/// Telegram accounts get the application credentials as their secret and
/// find their session in the store. Each is shown in its state; none stops
/// the rest.
fn start_telegram_accounts(
    system: &std::sync::Arc<System>,
    accounts: &accounts::Accounts,
    grant: &genatrix_connector::capability::Grant,
) -> anyhow::Result<()> {
    use genatrix_connector::SyncState;

    if accounts.telegram.is_empty() {
        return Ok(());
    }
    let credentials = match genatrix_connector_telegram::Credentials::find() {
        Ok(c) => c,
        Err(e) => {
            for account in accounts.telegram.values() {
                system
                    .accounts
                    .track(&account.phone)
                    .send_replace(SyncState::Stopped {
                        detail: format!("no Telegram application credentials: {e}"),
                    });
            }
            return Ok(());
        }
    };
    let secret = serde_json::to_string(&credentials)?;
    let assigned: Vec<connectors::Assigned> = accounts
        .telegram
        .values()
        .filter_map(|account| {
            let status = system.accounts.track(&account.phone);
            grant
                .account(&account.phone)
                .map(|capability| connectors::Assigned {
                    capability: capability.clone(),
                    secret: secret.clone(),
                    status,
                })
        })
        .collect();
    if let Some(binary) = connectors::binary_for("telegram") {
        return connectors::start_telegram(system.clone(), assigned, binary);
    }
    for a in &assigned {
        a.status.send_replace(SyncState::Stopped {
            detail:
                "genatrix-telegram is not beside this binary; `cargo build --release --workspace`"
                    .into(),
        });
    }
    Ok(())
}

/// Serve the interface and keep every account up to date while it runs.
///
/// The mail connector runs in its own sandboxed process (design 05) when
/// its binary is beside this one; the core hands it the accounts and their
/// passwords over the socket and stores what comes back. Without the binary,
/// as in a partial development build, the engine runs inside this process,
/// unconfined, and says so. An account that cannot start, because there is
/// no password or its server is not allowed, is shown in that state rather
/// than stopping the rest.
async fn serve(config: &Config, serving: &web::Serving) -> anyhow::Result<()> {
    use genatrix_connector::SyncState;
    use genatrix_connector::capability::Host;
    use genatrix_connector_imap::{Credentials, Imap, run_account};

    let key = ticket_key(config, false)?;
    let system = std::sync::Arc::new(System::open(config.clone(), key.clone())?);
    leave_ticket_key(config, &key)?;
    models::start(system.clone(), &key)?;
    tokio::spawn(pipelines_as_mail_arrives(system.clone()));

    let accounts = accounts::Accounts::load(&config.accounts_path())?;
    let grant = accounts.grant();

    let mut assigned = Vec::new();
    for account in accounts.mail.values() {
        let status = system.accounts.track(&account.address);
        let secret = match password_for(&account.address) {
            Ok(Some(password)) => password,
            Ok(None) => {
                status.send_replace(SyncState::NeedsLogin {
                    detail: format!(
                        "no password stored; run `genatrix account --add {}`",
                        account.address
                    ),
                });
                continue;
            }
            Err(e) => {
                status.send_replace(SyncState::Retrying {
                    detail: format!("the keychain could not be read: {e}"),
                    attempt: 0,
                    next_in_secs: 0,
                });
                tracing::warn!(account = %account.address, error = %e, "keychain");
                continue;
            }
        };
        let host = Host::new(&account.imap_host, account.imap_port);
        let capability = match grant.account(&account.address) {
            Some(c) if c.may_reach(&host) => c.clone(),
            _ => {
                status.send_replace(SyncState::Stopped {
                    detail: format!("this account is not allowed to connect to {host}"),
                });
                continue;
            }
        };
        assigned.push(connectors::Assigned {
            capability,
            secret,
            status,
        });
    }

    start_telegram_accounts(&system, &accounts, &grant)?;

    if let Some(binary) = connectors::mail_binary() {
        connectors::start_mail(system.clone(), assigned, binary)?;
    } else {
        {
            tracing::warn!(
                "genatrix-imap was not found beside this binary; running the mail connector \
                 inside the core, without a sandbox. `cargo build --workspace` builds it."
            );
            for connectors::Assigned {
                capability,
                secret,
                status,
            } in assigned
            {
                let Some(imap) = capability.hosts.iter().find(|h| h.port == 993).cloned() else {
                    status.send_replace(SyncState::Stopped {
                        detail: "no mail server to read from".to_owned(),
                    });
                    continue;
                };
                let credentials = Credentials {
                    account: capability.account.clone(),
                    password: secret,
                    imap,
                };
                let sink = syncing::StoreSink::new(system.clone(), &capability.account);
                tokio::spawn(run_account(
                    capability,
                    move || {
                        let credentials = credentials.clone();
                        async move { Imap::connect(&credentials).await }
                    },
                    sink,
                    status,
                ));
            }
        }
    }

    web::serve(system, serving).await
}

fn timeline(config: &Config, limit: u32) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    let items = system.store.query_items(&ItemQuery {
        limit,
        version: ItemVersion::Current,
        ..Default::default()
    })?;
    if items.is_empty() {
        println!("nothing yet. `genatrix seed` puts sample items in.");
        return Ok(());
    }
    for item in items {
        let who = item
            .author
            .and_then(|a| system.store.get_person(a).ok().flatten())
            .map_or_else(|| "unknown".to_owned(), |p| p.display_name);
        let mark = match item.sensitivity {
            genatrix_model::Level::Public => "public  ",
            genatrix_model::Level::Personal => "personal",
            genatrix_model::Level::Secret => "SECRET  ",
        };
        println!(
            "{}  {mark}  {:<9} {:<18} {}",
            item.occurred_at
                .with_timezone(&chrono::Local)
                .format("%m-%d %H:%M"),
            item.source.connector.as_str(),
            truncate(&who, 18),
            truncate(&item.text.replace('\n', " "), 64)
        );
    }
    Ok(())
}

/// Design 01 fixes the shape of this: the model as JSONL, plus the raw
/// records and attachments as files. Not the database, the model, so another
/// implementation can read it. Plain text, which the output says out loud.
fn export(config: &Config, to: &std::path::Path) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    system.store.checkpoint()?;
    let summary = system.store.export_to(to)?;

    // The bytes come out decrypted: an export that needed Genatrix to read it
    // would not be an export.
    let mut files = 0usize;
    let mut bytes = 0usize;
    for (name, store) in [("raw", &system.raw_files), ("blobs", &system.blob_files)] {
        let dir = to.join(name);
        std::fs::create_dir_all(&dir)?;
        for hash in hashes_in(config, name) {
            let contents = store.get(&hash)?;
            bytes += contents.len();
            files += 1;
            std::fs::write(dir.join(hash.to_string()), contents)?;
        }
    }

    println!("wrote {}", to.display());
    println!(
        "  {} items, {} threads, {} people, {} annotations",
        summary.items, summary.threads, summary.persons, summary.annotations
    );
    println!("  {files} files, {bytes} bytes");
    println!();
    println!("This is your data in the clear, with no encryption. Put it somewhere safe.");
    Ok(())
}

/// Which files a store holds. The store is content-addressed, so the names on
/// disk are the whole index.
fn hashes_in(config: &Config, which: &str) -> Vec<genatrix_model::ContentHash> {
    let root = config.data_dir.join(which);
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && let Ok(hash) = name.parse()
            {
                out.push(hash);
            }
        }
    }
    out
}

fn ledger(config: &Config) -> anyhow::Result<()> {
    let system = System::open(config.clone(), ticket_key(config, false)?)?;
    let verified = system.ledger.verify()?;

    let egress = system.ledger.entries(&EntryFilter {
        kind: Some(kind::EGRESS.into()),
        limit: 10_000,
        ..Default::default()
    })?;
    let mut left_the_device = 0usize;
    let mut bytes_out = 0usize;
    for entry in &egress {
        if let Ok(record) = entry.decode::<genatrix_gate::gate::EgressRecord>()
            && let Some(payload) = record.payload
        {
            left_the_device += 1;
            bytes_out += payload.len();
        }
    }

    // The sentence design 06 asks the records page to open with.
    if bytes_out == 0 {
        println!("0 bytes have left this device.");
    } else {
        println!("{bytes_out} bytes left this device in {left_the_device} request(s).");
    }
    println!();
    println!("{} model call(s) recorded", egress.len());
    println!(
        "{} entr(ies) in the chain, verified to the root",
        verified.entries
    );
    Ok(())
}

fn truncate(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    if text.chars().count() > width {
        out.push('…');
    }
    out
}
