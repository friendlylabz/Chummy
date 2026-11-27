//! chummyd: runs Chummy agents unattended, exactly as configured in the
//! Chummy app's Configure chummyd window.
//!
//! - It reads one encrypted bundle (the whole configuration) when it starts
//!   and runs that until it stops: nothing can reconfigure it while it runs.
//! - It listens on nothing. People talk to agents through channels (the
//!   terminal, end-to-end encrypted Matrix rooms, and Telegram bots), which can only
//!   start runs of an agent's own loop or carry out commands.
//! - Agents work only in their own folders, `<jail dir>/<agent>/`, which
//!   are kept apart from the bundle, the password file and the state.
//! - How runs behave comes from the same crates as the app
//!   (`chummy-session`, `chummy-runner`, `chummy-flow`…).

mod channels;
mod checks;
mod cli;
mod inspect;
mod jobs;
mod password;
mod platform;
mod router;
mod saver;
mod state;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use chummy_log::system::{self as syslog, Level, SysEntry};
use chummy_runner::usage::UsageLedger;
use chummy_scheduler::{JobStore, Manager};
use chummy_session::{MemoryChats, Session};
use chummy_tools::ToolPaths;

use crate::cli::{CheckArgs, Cmd, RunArgs};
use crate::platform::{DaemonConfig, DaemonPlatform, DaemonServices, Inner, MemoryPaused};
use crate::router::{Input, Router};
use crate::saver::Saver;
use crate::state::StateDir;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chummyd: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    let result = match cmd {
        Cmd::Help => {
            println!("{}", cli::USAGE);
            Ok(())
        }
        Cmd::Version => {
            println!("chummyd {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Cmd::Check(a) => {
            start_logging();
            check(a)
        }
        Cmd::HashPassword(bundle) => hash_password(&bundle),
        Cmd::Run(a) => {
            start_logging();
            match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(run(a)),
                Err(e) => Err(format!("couldn't start: {e}")),
            }
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chummyd: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The system log goes to standard error (standard output is the terminal
/// channel's), with metadata only: it never holds message text.
fn start_logging() {
    let log = syslog::init_without_files();
    log.set_listener(|e: &SysEntry| {
        let at = jiff::Timestamp::from_millisecond(e.at as i64)
            .map(|t| t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();
        let level = match e.level {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        };
        let fields: Vec<String> = e.fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
        eprintln!(
            "{at} {level} {}: {}{}{}",
            e.source,
            e.message,
            if fields.is_empty() { "" } else { " " },
            fields.join(" ")
        );
    });
}

fn read_bundle(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("can't read the bundle {}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > chummy_bundle::MAX_BUNDLE_BYTES as u64 {
        return Err(format!("{} isn't a bundle", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("can't read the bundle {}: {e}", path.display()))
}

fn open_bundle(
    path: &std::path::Path,
    password_file: Option<&std::path::Path>,
) -> Result<chummy_bundle::Config, String> {
    let bytes = read_bundle(path)?;
    let header = chummy_bundle::header(&bytes).map_err(|e| e.to_string())?;
    syslog::info(
        "app",
        format!("Opening the bundle for instance {}", header.instance_id),
    );
    let key = password::key(&bytes, password_file)?;
    chummy_bundle::open(&bytes, &key).map_err(|e| e.to_string())
}

/// `chummyd hash-password`: the line a password file holds, once the
/// password has opened the bundle.
fn hash_password(bundle: &std::path::Path) -> Result<(), String> {
    let bytes = read_bundle(bundle)?;
    let header = chummy_bundle::header(&bytes).map_err(|e| e.to_string())?;
    let pw = password::ask()?;
    let key = chummy_bundle::BundleKey::from_password(&pw, &header.instance_id)
        .map_err(|e| e.to_string())?;
    drop(pw);
    chummy_bundle::open_unchecked(&bytes, &key).map_err(|e| e.to_string())?;
    println!("{}", *key.to_line());
    Ok(())
}

/// `chummyd check`: opens the bundle and says what's in it and what's wrong,
/// without running anything.
fn check(a: CheckArgs) -> Result<(), String> {
    let c = open_bundle(&a.bundle, a.password_file.as_deref())?;
    for line in inspect::describe(&c) {
        println!("{line}");
    }
    for w in &c.check().warnings {
        println!("Note: {w}");
    }
    println!("The bundle is fine.");
    Ok(())
}

fn channel_names(c: &chummy_bundle::Config) -> String {
    let mut names: Vec<String> = vec![];
    if c.channels.terminal.enabled {
        names.push("terminal".into());
    }
    if c.channels.matrix.is_some() {
        names.push("matrix".to_string());
    }
    match c.channels.telegram.len() {
        0 => {}
        1 => names.push("telegram (1 bot)".into()),
        n => names.push(format!("telegram ({n} bots)")),
    }
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

async fn run(a: RunArgs) -> Result<(), String> {
    checks::not_root()?;
    let config = Arc::new(open_bundle(&a.bundle, a.password_file.as_deref())?);
    #[cfg(not(feature = "matrix"))]
    if config.channels.matrix.is_some() {
        return Err(
            "the bundle has a Matrix channel, but this chummyd was built without Matrix".into(),
        );
    }
    #[cfg(not(feature = "telegram"))]
    if !config.channels.telegram.is_empty() {
        return Err(
            "the bundle has Telegram bots, but this chummyd was built without Telegram".into(),
        );
    }
    for w in config.check().warnings {
        syslog::warn("app", w);
    }

    // Where everything is, kept apart.
    let paths = checks::paths(&a.bundle, a.password_file.as_deref(), &a.state, &a.jail_dir)?;
    let protected = checks::protected(&paths);
    let jails = checks::jails(
        &paths.jail_dir,
        config.agents.iter().map(|a| a.id.as_str()),
        &protected,
    )?;
    // The agents' tools run inside chummyd, as its user: everything in
    // their folders should be its.
    syslog::info(
        "app",
        format!(
            "Running as {}; agents' tools run inside chummyd, as the same user",
            checks::running_as()
        ),
    );
    for (id, dir) in &jails {
        if let Some(w) = checks::ownership(id, dir)? {
            syslog::warn("app", w);
        }
    }
    let key = config
        .state_key_bytes()
        .ok_or("the bundle's state key isn't valid")?;
    #[cfg(feature = "matrix")]
    let matrix_key = config
        .state_key_bytes()
        .ok_or("the bundle's state key isn't valid")?;
    let (state, saved) = StateDir::open(&paths.state, key, &config.instance.id)?;
    let saved = saved.unwrap_or_else(|| state.blank());
    let tool_paths = ToolPaths::new(state.dir.join("tools"));
    #[cfg(feature = "matrix")]
    let matrix_store = state.dir.join("matrix");
    let kept: saver::Kept = {
        let mut k = std::collections::BTreeMap::new();
        if let Some(m) = &saved.matrix {
            k.insert(router::MATRIX.to_string(), m.clone());
        }
        for (agent, v) in &saved.telegram {
            k.insert(channels::telegram_id(agent), v.clone());
        }
        Arc::new(std::sync::Mutex::new(k))
    };

    let (inbox, mut rx) = tokio::sync::mpsc::unbounded_channel::<Input>();
    let p = DaemonPlatform(Arc::new(Inner {
        session: Session::new(Arc::new(UsageLedger::from_snapshot(saved.usage.clone()))),
        config: DaemonConfig::new(&config, &jails),
        chats: MemoryChats::from_snapshot(
            saved.chats.clone(),
            config.limits.chat_messages as usize,
            config.limits.chats as usize,
        ),
        paused: MemoryPaused::from_snapshot(&saved.paused),
        protected,
        services: Arc::new(DaemonServices::new(
            config.secrets.clone(),
            tool_paths.clone(),
        )),
        tool_paths: tool_paths.clone(),
        inbox: inbox.clone(),
        dirty: AtomicBool::new(false),
        bundle: config.clone(),
    }));
    // Paused runs kept from before: ones that don't open are removed.
    chummy_session::paused::load(&p);

    if config.permissions.tools {
        set_up_tools(p.clone(), tool_paths);
    }

    let store = JobStore::in_memory(config.jobs.clone(), saved.jobs.clone());
    let tz = jiff::tz::TimeZone::get(&config.instance.time_zone).ok();
    let manager = Manager::new(
        jobs::DaemonJobs { p: p.clone() },
        store.clone(),
        config.limits.max_runs as usize,
        tz,
    );
    let saver = Arc::new(Saver::new(p.clone(), state, store, kept.clone()));
    saver.save_now();
    let saving = saver.every();
    let scheduling = {
        let m = manager.clone();
        tokio::spawn(async move { m.run_forever().await })
    };

    // Questions, to the router.
    {
        let mut asks = p.0.session.asks.subscribe();
        let inbox = inbox.clone();
        tokio::spawn(async move {
            loop {
                match asks.recv().await {
                    Ok(e) => {
                        if inbox.send(Input::Ask(e)).is_err() {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                }
            }
        });
    }

    // The channels: Matrix, and a Telegram bot for each agent that has one.
    #[allow(unused_mut)]
    let mut handles: Vec<chummy_channel::Handle> = vec![];
    #[allow(unused_mut)]
    let mut senders = std::collections::BTreeMap::new();
    #[cfg(feature = "matrix")]
    if config.channels.matrix.is_some() {
        let h = channels::start_matrix(&p, &saver, &kept, matrix_store, matrix_key, &inbox);
        senders.insert(router::MATRIX.to_string(), h.sender());
        handles.push(h);
    }
    #[cfg(feature = "telegram")]
    for bot in &config.channels.telegram {
        let h = channels::start_telegram(&p, &saver, &kept, bot, &inbox)?;
        senders.insert(channels::telegram_id(&bot.agent), h.sender());
        handles.push(h);
    }
    let any_channel = !senders.is_empty();

    let quit = Arc::new(tokio::sync::Notify::new());
    let mut router = Router::new(
        p.clone(),
        manager.clone(),
        saver.clone(),
        senders,
        kept.clone(),
        quit.clone(),
    );
    let routing = tokio::spawn(async move {
        router.greet();
        while let Some(input) = rx.recv().await {
            router.handle(input).await;
        }
    });
    // The terminal: a channel, or (with Matrix or Telegram) where the
    // operator confirms verifications and lets people in.
    if config.channels.terminal.enabled || any_channel {
        router::read_terminal(inbox.clone());
    }
    syslog::info(
        "app",
        format!(
            "chummyd {} is running “{}”: {} agents, {} jobs, channels: {}",
            env!("CARGO_PKG_VERSION"),
            config.instance.name,
            config.agents.len(),
            config.jobs.len(),
            channel_names(&config)
        ),
    );

    wait_for_stop(&quit).await;

    // Shutting down: no new messages or jobs; chat runs pause (to carry on
    // with /resume next time), anything else stops.
    let _ = inbox.send(Input::ShuttingDown);
    scheduling.abort();
    let runs = p.0.session.pause_all_for_shutdown();
    if runs > 0 {
        syslog::info("app", format!("Shutting down: pausing {runs} runs"));
    }
    let deadline = Instant::now() + a.grace;
    while p.0.session.run_count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if p.0.session.run_count() > 0 {
        syslog::warn("app", "Runs didn't pause in time, so they were stopped");
        p.0.session.stop_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while p.0.session.run_count() > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    // Let the router show what the runs said last, and the channels send it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    for h in &handles {
        h.flush(Duration::from_secs(10)).await;
    }
    saving.abort();
    saver.save_now();
    routing.abort();
    syslog::info("app", "chummyd stopped");
    Ok(())
}

/// Until Ctrl-C, SIGTERM, or `/quit`.
async fn wait_for_stop(quit: &tokio::sync::Notify) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = quit.notified() => {}
                }
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = quit.notified() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = quit.notified() => {}
    }
}

/// Unpacks the tools' Python (built in) and the built-in tools into the
/// state folder, in the background. Until it's done (or if this build has
/// no Python), no tools are offered.
fn set_up_tools(p: DaemonPlatform, paths: ToolPaths) {
    tokio::spawn(async move {
        let result: Result<(), String> = async {
            let unpack = paths.clone();
            tokio::task::spawn_blocking(move || chummy_tools::bundled::install(&unpack))
                .await
                .map_err(|e| e.to_string())??;
            let host = p.0.services.tool_host().await?;
            chummy_tools::install_builtins(&host, &mut |_| {})
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => syslog::info("tools", "Tools are ready"),
            Err(e) => syslog::warn("tools", format!("Tools aren't available: {e}")),
        }
    });
}
