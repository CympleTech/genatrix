//! The terminal side of executors: setting up the executor's user, filing
//! projects, recipes and roles, drafting work, and signing in to GitHub.
//!
//! Design: `docs/design/13-executors.md`. A terminal on this machine reads
//! the data directory directly, so it is this machine (design 02, invariant
//! 11): adding a project, which decides which vendors see its code, is done
//! here or on the page from this machine, never from a paired phone.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use clap::Subcommand;
use genatrix_exec::isolation::{EXEC_GROUP, EXEC_USER};

use crate::system::System;

/// `genatrix exec`.
#[derive(Debug, Subcommand)]
pub enum ExecAction {
    /// Create the executor's system user, its group and directories, and the
    /// one rule that lets you start programs as it. Asks for your password
    /// once, through `sudo`, and shows every command first.
    Setup {
        /// Print the commands; run nothing.
        #[arg(long)]
        print: bool,
    },
    /// Say whether everything an executor needs is in place.
    Status,
    /// Sign an executor in, as the executor's user. Claude Code takes a
    /// token you make with `claude setup-token`; Codex runs its own login.
    Login {
        /// The executor, as named in executors.toml.
        executor: String,
    },
}

/// `genatrix project`, `genatrix recipe`, `genatrix role`.
#[derive(Debug, Subcommand)]
pub enum DefinitionAction {
    /// File one from a TOML file, checked first.
    Add {
        /// The file.
        file: PathBuf,
    },
    /// List what is on file.
    List,
    /// Stop using one. What it was stays on file for the work that named it.
    Remove {
        /// Its name.
        name: String,
    },
}

/// `genatrix work`.
#[derive(Debug, Subcommand)]
pub enum WorkAction {
    /// Draft a dispatch. It waits on the approvals page like any action.
    Draft {
        /// The project.
        project: String,
        /// The recipe.
        recipe: String,
        /// The task, in your words.
        task: String,
        /// Say the task is not private, so it may go to executors as it is.
        #[arg(long)]
        public: bool,
    },
    /// The latest dispatches.
    List,
    /// One dispatch: its steps and what the executors said.
    Show {
        /// The dispatch's id.
        id: String,
    },
    /// Offer a failed push again: the same commit to the same branch, as a
    /// new action that waits for your approval.
    PushAgain {
        /// The dispatch's id.
        id: String,
    },
    /// Answer the question a dispatch is waiting on. The running core
    /// carries the work on within a few seconds.
    Answer {
        /// The dispatch's id.
        id: String,
        /// Choose one of its options, counting from 1.
        #[arg(long)]
        choose: Option<usize>,
        /// Or answer in your own words.
        text: Option<String>,
        /// Say your words are not private, so they may go to the executor
        /// as they are.
        #[arg(long)]
        public: bool,
    },
    /// Stop a dispatch. A running one is stopped by the core running it; one
    /// waiting for your answer is stopped here.
    Stop {
        /// The dispatch's id.
        id: String,
    },
}

/// `genatrix github`.
#[derive(Debug, Subcommand)]
pub enum GithubAction {
    /// Keep a token for pushes and pull requests. Asks for it; checks it
    /// with GitHub; keeps it in the keychain.
    Login,
    /// Forget the token.
    Logout,
}

/// The commands `genatrix exec setup` runs, in order.
#[must_use]
pub fn setup_commands(root: &std::path::Path, me: &str, uid: u32, gid: u32) -> Vec<Vec<String>> {
    let root = root.to_string_lossy().into_owned();
    let user_path = format!("/Users/{EXEC_USER}");
    let group_path = format!("/Groups/{EXEC_GROUP}");
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let (uid, gid) = (uid.to_string(), gid.to_string());
    vec![
        s(&["dscl", ".", "-create", &group_path]),
        s(&["dscl", ".", "-create", &group_path, "PrimaryGroupID", &gid]),
        s(&["dscl", ".", "-create", &group_path, "RealName", "Genatrix"]),
        s(&["dscl", ".", "-create", &user_path]),
        s(&["dscl", ".", "-create", &user_path, "UniqueID", &uid]),
        s(&["dscl", ".", "-create", &user_path, "PrimaryGroupID", &gid]),
        s(&["dscl", ".", "-create", &user_path, "UserShell", "/bin/zsh"]),
        s(&[
            "dscl",
            ".",
            "-create",
            &user_path,
            "NFSHomeDirectory",
            &format!("{root}/home"),
        ]),
        s(&[
            "dscl",
            ".",
            "-create",
            &user_path,
            "RealName",
            "Genatrix executor",
        ]),
        s(&["dscl", ".", "-create", &user_path, "IsHidden", "1"]),
        s(&["dscl", ".", "-create", &user_path, "Password", "*"]),
        s(&[
            "dseditgroup",
            "-o",
            "edit",
            "-a",
            me,
            "-t",
            "user",
            EXEC_GROUP,
        ]),
        s(&[
            "mkdir",
            "-p",
            &format!("{root}/home"),
            &format!("{root}/exec"),
            &format!("{root}/mirrors"),
        ]),
        s(&[
            "chown",
            &format!("{EXEC_USER}:{EXEC_GROUP}"),
            &format!("{root}/home"),
            &format!("{root}/exec"),
        ]),
        s(&[
            "chmod",
            "0700",
            &format!("{root}/home"),
            &format!("{root}/exec"),
        ]),
        s(&[
            "chown",
            &format!("{me}:{EXEC_GROUP}"),
            &format!("{root}/mirrors"),
        ]),
        s(&["chmod", "0750", &format!("{root}/mirrors")]),
        s(&["chmod", "0755", &root]),
    ]
}

/// The one sudo rule: you may start programs as the executor's user, which
/// is a step down, never up.
#[must_use]
pub fn sudoers_rule(me: &str) -> String {
    format!(
        "# Genatrix: {me} may start programs as the executor's user (design 13).\n\
         {me} ALL=({EXEC_USER}) NOPASSWD: ALL\n"
    )
}

fn me() -> anyhow::Result<String> {
    let out = Command::new("/usr/bin/id").arg("-un").output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn free_id(kind: &str, attr: &str) -> anyhow::Result<u32> {
    let out = Command::new("/usr/bin/dscl")
        .args([".", "-list", kind, attr])
        .output()?;
    let used: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().last()?.parse().ok())
        .collect();
    (401..499)
        .find(|n| !used.contains(n))
        .ok_or_else(|| anyhow::anyhow!("no free id between 401 and 499 for {kind}"))
}

fn user_exists() -> bool {
    user_exists_named(EXEC_USER)
}

/// Whether a system user of this name exists.
#[must_use]
pub fn user_exists_named(name: &str) -> bool {
    Command::new("/usr/bin/id")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Carry out `genatrix exec`. Needs only the executor settings: setting up
/// the executor's user neither opens the store nor asks for the master key.
#[allow(clippy::too_many_lines)] // three commands, each read top to bottom
pub fn exec(exec: &super::Exec, action: ExecAction) -> anyhow::Result<()> {
    let config = exec.config();
    match action {
        ExecAction::Setup { print } => {
            let me = me()?;
            if user_exists() && !print {
                println!(
                    "{EXEC_USER} exists already. `genatrix exec status` says what is missing."
                );
                return Ok(());
            }
            let uid = free_id("/Users", "UniqueID")?;
            let gid = free_id("/Groups", "PrimaryGroupID")?;
            let commands = setup_commands(&config.root, &me, uid, gid);
            let rule = sudoers_rule(&me);
            println!("This creates the executor's user, which runs Claude Code and Codex so that");
            println!("they cannot read your files, your keychain or your git credentials:\n");
            for c in &commands {
                println!("  sudo {}", c.join(" "));
            }
            println!("\nand writes /etc/sudoers.d/genatrix (checked with visudo first):\n");
            print!("  {}", rule.replace('\n', "\n  "));
            println!();
            if print {
                return Ok(());
            }
            print!("Go ahead? [y/N] ");
            std::io::stdout().flush()?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !answer.trim().eq_ignore_ascii_case("y") {
                println!("Nothing was changed.");
                return Ok(());
            }
            for c in &commands {
                let status = Command::new("/usr/bin/sudo").args(c).status()?;
                if !status.success() {
                    anyhow::bail!("`sudo {}` failed; stopped there", c.join(" "));
                }
            }
            let staged =
                std::env::temp_dir().join(format!("genatrix-sudoers-{}", std::process::id()));
            std::fs::write(&staged, &rule)?;
            let checked = Command::new("/usr/bin/sudo")
                .args(["/usr/sbin/visudo", "-cf"])
                .arg(&staged)
                .status()?;
            if !checked.success() {
                let _ = std::fs::remove_file(&staged);
                anyhow::bail!("visudo did not accept the rule; nothing was installed");
            }
            let installed = Command::new("/usr/bin/sudo")
                .args(["install", "-m", "0440", "-o", "root", "-g", "wheel"])
                .arg(&staged)
                .arg("/etc/sudoers.d/genatrix")
                .status()?;
            if !installed.success() {
                anyhow::bail!("could not install the sudo rule");
            }
            let _ = std::fs::remove_file(&staged);
            println!("\nDone. Next: install Node for everyone (`brew install node`), then");
            for e in &config.executors {
                println!("  genatrix exec login {}", e.name);
            }
            Ok(())
        }
        ExecAction::Status => {
            let mut ok = true;
            let mut line = |good: bool, what: &str, fix: &str| {
                ok &= good;
                println!(
                    "{} {what}{}",
                    if good { "ok     " } else { "MISSING" },
                    if good {
                        String::new()
                    } else {
                        format!("  ({fix})")
                    }
                );
            };
            line(
                user_exists(),
                &format!("system user {EXEC_USER}"),
                "genatrix exec setup",
            );
            let sudo_ok = Command::new("/usr/bin/sudo")
                .args(["-n", "-u", EXEC_USER, "/usr/bin/true"])
                .output()
                .is_ok_and(|o| o.status.success());
            line(
                sudo_ok,
                "starting programs as it without a password",
                "genatrix exec setup",
            );
            for dir in ["exec", "mirrors", "home"] {
                line(
                    config.root.join(dir).is_dir(),
                    &format!("{}", config.root.join(dir).display()),
                    "genatrix exec setup",
                );
            }
            for e in &config.executors {
                let program = e.command.first().cloned().unwrap_or_default();
                let found = sudo_ok
                    && Command::new("/usr/bin/sudo")
                        .args([
                            "-n",
                            "-H",
                            "-u",
                            EXEC_USER,
                            "--",
                            "/usr/bin/env",
                            &format!("PATH={}", e.path),
                            "/bin/sh",
                            "-c",
                            "command -v \"$0\"",
                            &program,
                        ])
                        .output()
                        .is_ok_and(|o| o.status.success());
                line(
                    found,
                    &format!("`{program}` for {} ({})", e.name, e.vendor),
                    "install it where the executor's user finds it, such as `brew install node`",
                );
            }
            for e in config.executors.iter().filter(|e| e.token_env.is_some()) {
                let file = format!("$HOME/.genatrix/{}.env", e.name);
                let has = sudo_ok
                    && Command::new("/usr/bin/sudo")
                        .args(["-n", "-H", "-u", EXEC_USER, "--", "/bin/sh", "-c"])
                        .arg(format!("test -s \"{file}\""))
                        .output()
                        .is_ok_and(|o| o.status.success());
                line(
                    has,
                    &format!("a sign-in token for {}", e.name),
                    &format!("genatrix exec login {}", e.name),
                );
            }
            line(
                super::github::token().is_some(),
                "a GitHub token",
                "genatrix github login",
            );
            line(
                crate::connectors::binary_for("exec").is_some(),
                "genatrix-exec beside this program",
                "build the workspace",
            );
            line(
                crate::connectors::binary_for("github").is_some(),
                "genatrix-github beside this program",
                "build the workspace",
            );
            if ok {
                println!("\nEverything is in place.");
            }
            Ok(())
        }
        ExecAction::Login { executor } => {
            let Some(e) = config.executor(&executor) else {
                anyhow::bail!("no executor `{executor}` in executors.toml");
            };
            if let Some(var) = &e.token_env {
                return login_with_token(&e.name, var);
            }
            if e.login.is_empty() {
                anyhow::bail!("executors.toml gives no login command for `{executor}`");
            }
            println!(
                "Signing {executor} in as {EXEC_USER}. Its login is its own; Genatrix keeps nothing of it."
            );
            let status = Command::new("/usr/bin/sudo")
                .args([
                    "-H",
                    "-u",
                    EXEC_USER,
                    "--",
                    "/usr/bin/env",
                    &format!("PATH={}", e.path),
                ])
                .args(&e.login)
                .status()?;
            if !status.success() {
                anyhow::bail!("the login did not finish");
            }
            Ok(())
        }
    }
}

/// Whether a token can be written into a shell assignment as it is: the
/// file holding it is read by the executor's shell, so nothing in it may
/// be shell text.
fn token_is_plain(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Ask for a sign-in token and leave it in the executor's user's home,
/// readable by that user only. It goes to `sudo` on standard input, never
/// on a command line.
fn login_with_token(name: &str, var: &str) -> anyhow::Result<()> {
    println!("{name} signs in with a long-lived token, made as yourself:");
    println!();
    println!("  claude setup-token");
    println!();
    println!("Run that in another terminal, finish in the browser, and paste the token here.");
    println!("It is kept in {EXEC_USER}'s home only; Genatrix keeps no copy.");
    let token = rpassword::prompt_password("Token: ")?;
    let token = token.trim();
    if !token_is_plain(token) {
        anyhow::bail!("that does not look like a token; nothing was stored");
    }
    let mut child = Command::new("/usr/bin/sudo")
        .args(["-n", "-H", "-u", EXEC_USER, "--", "/bin/sh", "-c"])
        .arg(
            "umask 077 && mkdir -p \"$HOME/.genatrix\" && cat > \"$HOME/.genatrix/$0.env.tmp\" \
             && mv \"$HOME/.genatrix/$0.env.tmp\" \"$HOME/.genatrix/$0.env\"",
        )
        .arg(name)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin for sudo"))?
        .write_all(format!("{var}={token}\n").as_bytes())?;
    if !child.wait()?.success() {
        anyhow::bail!("could not store the token as {EXEC_USER}; run `genatrix exec setup` first");
    }
    println!("Stored. {name} starts signed in from the next step on.");
    Ok(())
}

/// Carry out `genatrix project|recipe|role`.
pub fn definitions(system: &System, kind: &str, action: DefinitionAction) -> anyhow::Result<()> {
    match action {
        DefinitionAction::Add { file } => {
            let text = std::fs::read_to_string(&file)?;
            let (name, hash) = super::file_definition(&system.store, kind, &text)?;
            println!("{kind} `{name}` filed ({})", &hash[..12]);
            if kind == "project" {
                let p = genatrix_dispatch::project::Project::parse(&text)?;
                println!(
                    "Its code may be seen by: {} (each sends what it is given to its vendor).",
                    p.executors.join(", ")
                );
            }
        }
        DefinitionAction::List => {
            super::seed(&system.store)?;
            for d in system.store.active_definitions(kind)? {
                println!("{:<24} {}", d.name, &d.hash[..12]);
            }
        }
        DefinitionAction::Remove { name } => {
            if system.store.retire_definition(kind, &name)? {
                println!("{kind} `{name}` is no longer used");
            } else {
                println!("no {kind} `{name}`");
            }
        }
    }
    Ok(())
}

/// Carry out `genatrix work`.
#[allow(clippy::too_many_lines)] // one match, one arm per command
pub fn work(system: &System, action: WorkAction) -> anyhow::Result<()> {
    match action {
        WorkAction::Draft {
            project,
            recipe,
            task,
            public,
        } => {
            super::seed(&system.store)?;
            let a = super::draft::draft(
                system,
                &super::draft::Request {
                    project,
                    recipe,
                    task,
                    public,
                },
            )?;
            println!("Drafted {}. Approve it on the approvals page.", a.id);
        }
        WorkAction::List => {
            for d in system.store.list_dispatches(None, 20)? {
                println!(
                    "{}  {:<9} {:<16} {:<22} {}",
                    d.id,
                    d.state,
                    d.project,
                    d.recipe,
                    d.detail.unwrap_or_default()
                );
            }
        }
        WorkAction::Show { id } => {
            let Some(d) = system.store.get_dispatch(&id)? else {
                anyhow::bail!("no dispatch {id}");
            };
            println!("{} · {} · {} · {}", d.project, d.recipe, d.state, d.level);
            if let Some(detail) = &d.detail {
                println!("{detail}");
            }
            if let Some((_, q)) = super::run::question(system, &id)? {
                println!("\n{}\n\n{}", q.progress, q.question);
                for (i, o) in q.options.iter().enumerate() {
                    let mark = if q.recommend == Some(i) {
                        " (recommended)"
                    } else {
                        ""
                    };
                    println!("  {}. {o}{mark}", i + 1);
                }
                println!("\ngenatrix work answer {id} --choose N, or in your own words");
            }
            for s in system.store.dispatch_steps(&id)? {
                println!(
                    "\n#{} {} (round {}) · {} as {} · {}",
                    s.seq, s.step, s.round, s.executor, s.role, s.state
                );
                if let Some(e) = &s.error {
                    println!("  error: {e}");
                }
                if let Some(c) = &s.commit_id {
                    println!("  commit {c}");
                }
            }
            for e in system.store.dispatch_events(&id, None, 500)? {
                println!("  [{}] {}: {}", e.seq, e.kind, e.text.replace('\n', " "));
            }
        }
        WorkAction::PushAgain { id } => match super::github::push_again(system, &id)? {
            Ok(a) => println!("Offered again as {}; it waits on the approvals page.", a.id),
            Err(why) => anyhow::bail!(why),
        },
        WorkAction::Answer {
            id,
            choose,
            text,
            public,
        } => {
            let reply = match (choose, text) {
                (Some(n), None) => {
                    let Some((_, q)) = super::run::question(system, &id)? else {
                        anyhow::bail!("{id} is not waiting for an answer");
                    };
                    let Some(o) = n.checked_sub(1).and_then(|i| q.options.get(i)) else {
                        anyhow::bail!("there is no option {n}");
                    };
                    super::run::Reply {
                        text: o.clone(),
                        public: true,
                    }
                }
                (None, Some(text)) => super::run::Reply { text, public },
                _ => anyhow::bail!("give either --choose N or your own words"),
            };
            match super::run::answer(system, &id, &reply)? {
                Ok(()) => println!("Answered. The core carries the work on within a few seconds."),
                Err(why) => anyhow::bail!(why),
            }
        }
        WorkAction::Stop { id } => {
            if super::run::stop_waiting(system, &id)? {
                println!("Stopped; its workspace is gone.");
                return Ok(());
            }
            println!(
                "A dispatch is stopped by the core running it: use the stop button on its page \
                 (the core's local API: POST /api/dispatch/{id}/stop)."
            );
        }
    }
    Ok(())
}

/// Carry out `genatrix github`.
pub async fn github(system: Arc<System>, action: GithubAction) -> anyhow::Result<()> {
    use super::github::{ACCOUNT, SERVICE};
    let chain = crate::keychain::Keychain::named(SERVICE);
    match action {
        GithubAction::Login => {
            println!("A fine-grained token with Contents and Pull requests (read and write) on");
            println!("the repositories your projects name is enough.");
            let token = rpassword::prompt_password("GitHub token: ")?;
            let token = token.trim().to_owned();
            let login = super::github::whoami(&system, &token).await?;
            chain.store(ACCOUNT, &token)?;
            println!("Signed in as {login}. The token is in the keychain.");
        }
        GithubAction::Logout => {
            if chain.forget(ACCOUNT)? {
                println!("The token is gone from the keychain.");
            } else {
                println!("There was no token.");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_stored_only_when_it_cannot_be_shell_text() {
        assert!(token_is_plain("sk-ant-oat01-AbC_123.x"));
        assert!(!token_is_plain(""));
        assert!(!token_is_plain("a b"));
        assert!(!token_is_plain("x;rm -rf ~"));
        assert!(!token_is_plain("$(id)"));
        assert!(!token_is_plain("a'b"));
    }

    #[test]
    fn setup_makes_a_hidden_user_with_no_password_and_a_narrow_rule() {
        let commands = setup_commands(
            std::path::Path::new("/Users/Shared/Genatrix"),
            "neo",
            410,
            410,
        );
        let flat: Vec<String> = commands.iter().map(|c| c.join(" ")).collect();
        assert!(flat.iter().any(|c| c.contains("IsHidden 1")));
        assert!(
            flat.iter().any(|c| c.ends_with("Password *")),
            "no password: nobody logs in as it"
        );
        assert!(
            flat.iter()
                .any(|c| c == "chmod 0700 /Users/Shared/Genatrix/home /Users/Shared/Genatrix/exec")
        );
        assert!(
            flat.iter()
                .any(|c| c == "chmod 0750 /Users/Shared/Genatrix/mirrors")
        );
        let rule = sudoers_rule("neo");
        assert!(rule.contains(&format!("neo ALL=({EXEC_USER}) NOPASSWD: ALL")));
        assert!(!rule.contains("(ALL)"), "never as anyone else, never root");
    }
}
