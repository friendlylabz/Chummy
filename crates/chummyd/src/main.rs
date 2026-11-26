//! chummyd: runs Chummy agents unattended, exactly as configured in the
//! Chummy app's Configure chummyd window.
//!
//! - It reads one encrypted bundle (the whole configuration) when it starts
//!   and runs that until it stops: nothing can reconfigure it while it runs.
//! - It listens on nothing. People talk to agents through channels (the
//!   terminal now; Matrix next), which can only start runs of an agent's
//!   own loop or carry out commands.
//! - Agents work only in their own folders, `<jail dir>/<agent>/`, which
//!   are kept apart from the bundle, the password file and the state.
//! - How runs behave comes from the same crates as the app
//!   (`chummy-session`, `chummy-runner`, `chummy-flow`…).

mod checks;
mod cli;
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
    let pw = password::read(password_file)?;
    // The password is dropped (and wiped) once the key is derived.
    chummy_bundle::open(&bytes, &pw).map_err(|e| e.to_string())
}

/// `chummyd check`: opens the bundle and says what's in it and what's wrong.
fn check(a: CheckArgs) -> Result<(), String> {
    let c = open_bundle(&a.bundle, a.password_file.as_deref())?;
    let r = c.check();
    println!(
        "Instance “{}” ({}), time zone {}",
        c.instance.name, c.instance.id, c.instance.time_zone
    );
    for ag in &c.agents {
        let jobs = c.jobs.iter().filter(|j| j.agent == ag.id).count();
        println!("  agent {} [{}]: {jobs} jobs", ag.name, ag.id);
    }
    println!(
        "  channels: {}",
        if c.channels.terminal.enabled {
            "terminal"
        } else {
            "none"
        }
    );
    for w in &r.warnings {
        println!("  note: {w}");
    }
    println!("The bundle is fine.");
    Ok(())
}

async fn run(a: RunArgs) -> Result<(), String> {
    checks::not_root()?;
    let config = Arc::new(open_bundle(&a.bundle, a.password_file.as_deref())?);
    for w in config.check().warnings {
        syslog::warn("app", w);
    }

    // Where everything is, kept apart.
    let paths = checks::paths(
        &a.bundle,
        a.password_file.as_deref(),
        a.state.as_deref(),
        &a.jail_dir,
    )?;
    let protected = checks::protected(&paths);
    let jails = checks::jails(
        &paths.jail_dir,
        config.agents.iter().map(|a| a.id.as_str()),
        &protected,
    )?;
    let key = config
        .state_key_bytes()
        .ok_or("the bundle's state key isn't valid")?;
    let (state, saved) = StateDir::open(&paths.state, key, &config.instance.id)?;
    let saved = saved.unwrap_or_else(|| state.blank());
    let tool_paths = ToolPaths::new(state.dir.join("tools"));

    let (inbox, mut rx) = tokio::sync::mpsc::unbounded_channel::<Input>();
    let p = DaemonPlatform(Arc::new(Inner {
        session: Session::new(Arc::new(UsageLedger::from_snapshot(saved.usage.clone()))),
        config: DaemonConfig::new(&config, &jails),
        chats: MemoryChats::from_snapshot(
            saved.chats.clone(),
            platform::MAX_MESSAGES,
            platform::MAX_CHATS,
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
    let saver = Arc::new(Saver::new(p.clone(), state, store));
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

    let quit = Arc::new(tokio::sync::Notify::new());
    let mut router = Router::new(p.clone(), manager.clone(), saver.clone(), quit.clone());
    let routing = tokio::spawn(async move {
        router.greet();
        while let Some(input) = rx.recv().await {
            router.handle(input).await;
        }
    });
    if config.channels.terminal.enabled {
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
            if config.channels.terminal.enabled {
                "terminal"
            } else {
                "none"
            }
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
    // Let the router show what the runs said last.
    tokio::time::sleep(Duration::from_millis(300)).await;
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
