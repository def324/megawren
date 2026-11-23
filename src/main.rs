mod backend;
mod bridge;
mod cache;
mod cli;
mod display;
mod error;
mod feedback;
mod input;
mod manage;
mod operation;
mod preview;
mod startup;
mod state;
mod text;
mod transfer;
mod tui;
mod vault;
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use feedback::Confirmation;
use operation::Stage;
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroizing;
#[derive(Parser)]
#[command(
    version,
    about = "MEGA file transfers. Run megawren without a command to open the file browser.",
    after_help = "Use ls --json for scripts. Progress stays hidden for JSON listings and whenever stdin, stdout, or stderr is redirected."
)]
struct Args {
    #[arg(
        long,
        global = true,
        help = "Read the local unlock passphrase from one line of redirected stdin"
    )]
    unlock_stdin: bool,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Preview a small remote text file or image on request
    Preview { remote: String },
    /// Sign in and choose whether to save the login
    Login,
    /// List a remote directory
    Ls {
        #[arg(default_value = "/")]
        remote: String,
        /// Write a JSON array to stdout with name, type and size in bytes (null for folders or unknown sizes)
        #[arg(long)]
        json: bool,
    },
    /// Download files or folders into a local directory
    #[command(
        override_usage = "megawren get [OPTIONS] [REMOTE_OR_LINK]\n       megawren get [OPTIONS] <REMOTE>... <LOCAL_DIR>"
    )]
    Get {
        /// One path downloads here. With two or more paths, the last is an existing local directory.
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// Replace existing local files, including files inside downloaded folders.
        #[arg(long)]
        overwrite: bool,
    },
    /// Upload files or folders into a remote directory
    #[command(
        override_usage = "megawren put [OPTIONS] <LOCAL>\n       megawren put [OPTIONS] <LOCAL>... <REMOTE_DIR>"
    )]
    Put {
        /// One path uploads to /. With two or more paths, the last is an existing remote directory.
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
    },
    /// Create remote folders
    Mkdir {
        /// Also create missing parent folders, and accept folders that already exist
        #[arg(short, long)]
        parents: bool,
        #[arg(required = true, value_name = "REMOTE")]
        paths: Vec<String>,
    },
    /// Move or rename remote files and folders
    #[command(
        override_usage = "megawren mv <REMOTE> <NEW_PATH>\n       megawren mv <REMOTE>... <REMOTE_DIR>"
    )]
    Mv {
        /// With two or more paths, the last is an existing remote folder. Existing items are never replaced.
        #[arg(required = true, num_args = 2.., value_name = "PATH")]
        paths: Vec<String>,
    },
    /// Copy remote files and folders
    #[command(
        override_usage = "megawren cp <REMOTE> <NEW_PATH>\n       megawren cp <REMOTE>... <REMOTE_DIR>"
    )]
    Cp {
        /// With two or more paths, the last is an existing remote folder. Existing items are never replaced.
        #[arg(required = true, num_args = 2.., value_name = "PATH")]
        paths: Vec<String>,
    },
    /// Move remote files and folders to the Rubbish Bin
    Rm {
        /// Also move folders and everything in them
        #[arg(short, long)]
        recursive: bool,
        #[arg(required = true, value_name = "REMOTE")]
        paths: Vec<String>,
    },
    /// Revoke and remove the saved login
    Logout,
    /// Show local state locations
    Status,
}
pub fn line(label: &str, cancel: &AtomicBool) -> Result<String> {
    if let Some(value) = feedback::read(label, false, 32768, None, cancel) {
        return Ok(value?.to_string());
    }
    Ok(input::read(label, false, 32768, cancel)?.to_string())
}
pub fn prompt(label: &str, cancel: &AtomicBool) -> Result<Zeroizing<String>> {
    prompt_with_error(label, None, cancel)
}
fn prompt_with_error(
    label: &str,
    error: Option<&'static str>,
    cancel: &AtomicBool,
) -> Result<Zeroizing<String>> {
    if let Some(value) = feedback::read(label, true, 4096, error, cancel) {
        return value;
    }
    if let Some(message) = error {
        eprintln!("{message}");
    }
    input::read(label, true, 4096, cancel)
}
fn unlock(
    stdin: bool,
    automatic: bool,
    error: Option<&'static str>,
    cancel: &AtomicBool,
) -> Result<Zeroizing<String>> {
    if automatic {
        return Ok(Zeroizing::new(String::new()));
    }
    if !stdin {
        ensure!(
            input::can_prompt() || feedback::active(),
            "Saved login is locked. Use --unlock-stdin and supply the local passphrase on stdin."
        );
        return prompt_with_error("Unlock saved login: ", error, cancel);
    }
    ensure!(
        !io::stdin().is_terminal(),
        "--unlock-stdin requires redirected input"
    );
    input::read_stdin_line(4097, cancel)
}
fn confirm_save(choice: Confirmation, cancel: &AtomicBool) -> Result<bool> {
    if let Some(answer) = feedback::confirm(choice, cancel) {
        return answer;
    }
    let question = if choice == Confirmation::WithoutPassphrase {
        eprintln!("{}", choice.question());
        "Save without a passphrase?"
    } else {
        choice.question()
    };
    loop {
        let answer = line(&format!("{question} [y/N]: "), cancel)?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => feedback::note("Enter y or n."),
        }
    }
}
fn account_login(backend: &mut backend::Backend, cancel: &AtomicBool) -> Result<()> {
    let email = line("Email: ", cancel)?;
    let password = prompt("Password: ", cancel)?;
    backend.login(&email, &password)
}
fn saved_login_passphrase(
    replacing: bool,
    cancel: &AtomicBool,
) -> Result<Option<Zeroizing<String>>> {
    let choice = if replacing {
        Confirmation::Replace
    } else {
        Confirmation::Remember
    };
    while confirm_save(choice, cancel)? {
        match choose_passphrase(cancel) {
            Ok(pass) => return Ok(Some(pass)),
            Err(e) if e.is::<feedback::Back>() => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}
fn choose_passphrase(cancel: &AtomicBool) -> Result<Zeroizing<String>> {
    let mut error = None;
    loop {
        let pass = prompt_with_error("Local passphrase (optional): ", error.take(), cancel)?;
        if pass.is_empty() {
            if confirm_save(Confirmation::WithoutPassphrase, cancel)? {
                return Ok(pass);
            }
            continue;
        }
        match prompt("Confirm local passphrase: ", cancel) {
            Ok(confirmation) if *confirmation == *pass => return Ok(pass),
            Ok(_) => error = Some("Passphrases didn’t match. Try again."),
            Err(e) if e.is::<feedback::Back>() => continue,
            Err(e) => return Err(e),
        }
    }
}
fn save_login(
    state: &state::State,
    backend: &mut backend::Backend,
    pass: &str,
    cancel: &AtomicBool,
) -> Result<()> {
    let session = backend.session()?;
    feedback::progress(Stage::SavingLogin, 0, 0, 0, false);
    let sealed = vault::seal(pass.as_bytes(), session.as_bytes())?;
    input::check(cancel)?;
    state.save(&sealed)?;
    backend.remember_session(session.as_bytes())?;
    cache::remove(&state.path)?;
    Ok(())
}
fn run(args: Args) -> Result<()> {
    let path = state::home()?;
    if matches!(args.command, Some(Command::Status)) {
        println!(
            "Saved login: {} ({})",
            state::display(&path.join("session.enc").display().to_string()),
            if path.join("session.enc").try_exists()? {
                if vault::opens_automatically(&state::State::at(path.clone())?.read_vault()?) {
                    "no passphrase"
                } else {
                    "locked"
                }
            } else {
                "none"
            }
        );
        println!(
            "Working data: {}",
            state::display(&path.display().to_string())
        );
        println!(
            "Local index: {} ({})",
            state::display(&path.join("cache.enc").display().to_string()),
            if path.join("cache.enc").try_exists()? {
                "encrypted"
            } else {
                "none"
            }
        );
        if path.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with("run-") {
                    println!(
                        "SDK working directory: {}",
                        state::display(&entry.path().display().to_string())
                    );
                }
            }
        }
        return Ok(());
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || {
        input::interrupt(&signal);
    })?;
    if args.command.is_none() || matches!(args.command, Some(Command::Preview { .. })) {
        return tui::application(cancelled.clone(), || run_account(args, cancelled));
    }
    operation::configure_cli_progress(matches!(args.command, Some(Command::Ls { json: true, .. })));
    run_account(args, cancelled)
}
fn run_account(mut args: Args, cancelled: Arc<AtomicBool>) -> Result<()> {
    let state = state::State::open()?;
    // Discover the saved-login state under a shared lock. First-time setup
    // needs exclusive access before offering to create the vault.
    let mut account_lock = state.lock(false)?;
    if matches!(args.command, Some(Command::Login | Command::Logout))
        || !state.path.join("session.enc").try_exists()?
    {
        drop(account_lock);
        account_lock = state.lock(true)?;
    }
    let transfer = cli::Transfer::prepare(args.command.as_mut(), &state.path, &cancelled)?;
    let public = transfer
        .as_ref()
        .and_then(cli::Transfer::public_link)
        .is_some();
    if !input::can_prompt() && !feedback::active() {
        ensure!(
            !matches!(args.command, Some(Command::Login)),
            "Run megawren login in a terminal."
        );
        ensure!(
            public || state.path.join("session.enc").try_exists()?,
            "No saved login. Run megawren login in a terminal."
        );
    }
    let mut backend = backend::Backend::deferred(&state, cancelled.clone())?;
    let result = (|| -> Result<()> {
        let mut startup_feedback = None;
        if let Some(link) = transfer.as_ref().and_then(cli::Transfer::public_link) {
            backend.initialize()?;
            startup_feedback = Some(operation::StartupFeedback::begin());
            if link.starts_with("https://mega.nz/folder/") {
                backend.api().public_folder(link)?;
                backend.wait()?;
                backend.api().fetch()?;
                backend.wait_for(Stage::LoadingShared)?;
            } else {
                ensure!(
                    link.starts_with("https://mega.nz/file/"),
                    "Unsupported MEGA link format"
                );
                backend.api().public_file(link)?;
                backend.wait()?;
            }
        } else if state.path.join("session.enc").try_exists()?
            && !matches!(args.command, Some(Command::Login))
        {
            let data = state.read_vault()?;
            let automatic = vault::opens_automatically(&data);
            let mut error = None;
            let session = loop {
                let pass = match unlock(args.unlock_stdin, automatic, error, &cancelled) {
                    Ok(pass) => pass,
                    Err(e) if e.is::<feedback::SignInAgain>() => {
                        account_lock.unlock()?;
                        account_lock = state.lock(true)?;
                        account_login(&mut backend, &cancelled)?;
                        break None;
                    }
                    Err(e) => return Err(e),
                };
                startup_feedback = Some(operation::StartupFeedback::begin());
                feedback::progress(Stage::UnlockingLogin, 0, 0, 0, false);
                match vault::open(pass.as_bytes(), &data) {
                    Ok(session) => break Some(session),
                    Err(e) if automatic => {
                        return Err(e.context(
                            "Cannot read saved login. Run megawren login to sign in again.",
                        ));
                    }
                    Err(_) if feedback::active() && !args.unlock_stdin => {
                        error = Some("Passphrase not accepted. Try again.");
                    }
                    Err(e) => return Err(e.context(error::UnlockSavedLogin)),
                }
            };
            if let Some(session) = session {
                input::check(&cancelled)?;
                let restored = backend.restore(
                    std::str::from_utf8(&session)?,
                    !matches!(args.command, Some(Command::Logout)),
                );
                if let Err(e) = restored {
                    if !e.is::<error::ExpiredSession>() {
                        return Err(e);
                    }
                    if matches!(args.command, Some(Command::Logout)) {
                        std::fs::remove_file(state.path.join("session.enc"))?;
                        cache::remove(&state.path)?;
                        feedback::note("Expired saved login removed.");
                        return Ok(());
                    }
                    if !feedback::active()
                        || !feedback::confirm(Confirmation::Expired, &cancelled)
                            .context("Interactive sign-in unavailable")??
                    {
                        return Err(e);
                    }
                    account_lock.unlock()?;
                    account_lock = state.lock(true)?;
                    backend = backend::Backend::deferred(&state, cancelled.clone())?;
                    account_login(&mut backend, &cancelled)?;
                }
            }
        } else {
            ensure!(
                !matches!(args.command, Some(Command::Logout)),
                "No saved login"
            );
            account_login(&mut backend, &cancelled)?;
        }
        if !public {
            let purpose = match &args.command {
                Some(Command::Login) => startup::Purpose::Login,
                Some(Command::Logout) => startup::Purpose::Logout,
                _ => startup::Purpose::UseFiles,
            };
            for step in startup::steps(purpose, !backend.remembered) {
                match step {
                    startup::Step::OfferRemember => {
                        let replacing = state.path.join("session.enc").try_exists()?;
                        if let Some(pass) = saved_login_passphrase(replacing, &cancelled)? {
                            save_login(&state, &mut backend, &pass, &cancelled)?;
                            if replacing {
                                feedback::note(
                                    "You can end the previous session in MEGA’s session settings.",
                                );
                            }
                        } else if matches!(purpose, startup::Purpose::Login) {
                            feedback::note(if replacing {
                                "Kept the previous saved login."
                            } else {
                                "Login not saved."
                            });
                        }
                    }
                    startup::Step::LoadFiles => {
                        if startup_feedback.is_none() {
                            startup_feedback = Some(operation::StartupFeedback::begin());
                        }
                        if matches!(
                            args.command,
                            Some(
                                Command::Ls { .. }
                                    | Command::Get { .. }
                                    | Command::Put { .. }
                                    | Command::Preview { .. }
                                    | Command::Mkdir { .. }
                                    | Command::Mv { .. }
                                    | Command::Cp { .. }
                                    | Command::Rm { .. }
                            )
                        ) {
                            backend.load_current_drive()?;
                        } else {
                            backend.load_drive()?;
                        }
                    }
                }
            }
        }
        drop(startup_feedback);
        if let Some(transfer) = transfer {
            return match transfer {
                cli::Transfer::Get {
                    sources,
                    destination,
                    overwrite,
                } => {
                    let entries = if public {
                        let entry = if sources[0].starts_with("https://mega.nz/file/") {
                            backend.api().public_entry()?
                        } else {
                            let root = backend.api().root()?;
                            backend.api().info(root)?
                        };
                        vec![entry]
                    } else {
                        backend.download_entries(sources.iter().map(|path| path.as_str()))?
                    };
                    tui::download(&mut backend, entries, destination, overwrite)
                }
                cli::Transfer::Put {
                    sources,
                    destination,
                } => {
                    let parent = backend.resolve(&destination)?;
                    tui::upload(&mut backend, sources, parent, destination)
                }
            };
        }
        match args.command {
            Some(Command::Preview { remote }) => {
                let id = backend.resolve(&remote)?;
                let entry = backend.api().info(id)?;
                preview::plan(&entry.name, entry.size, entry.folder)?;
                feedback::preview(&mut backend, entry)?;
            }
            Some(Command::Login) => (),
            Some(Command::Logout) => {
                backend.revoke()?;
                std::fs::remove_file(state.path.join("session.enc"))?;
                cache::remove(&state.path)?;
                eprintln!("Saved login revoked and removed.");
            }
            Some(Command::Ls { remote, json }) => {
                let epoch = backend.api().generation();
                let id = backend.resolve(&remote)?;
                let stdout = io::stdout();
                let result =
                    display::directory(&mut stdout.lock(), stdout.is_terminal(), json, |off| {
                        let rows = backend.children(id, off)?;
                        backend.check_generation(epoch)?;
                        Ok(rows)
                    });
                if let Err(e) = result
                    && !e
                        .downcast_ref::<io::Error>()
                        .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
                {
                    return Err(e);
                }
            }
            Some(Command::Mkdir { paths, parents }) => {
                manage::mkdir(&mut backend, &paths, parents)?
            }
            Some(Command::Mv { mut paths }) => {
                let to = paths.pop().unwrap();
                manage::relocate(&mut backend, &paths, &to, false)?;
            }
            Some(Command::Cp { mut paths }) => {
                let to = paths.pop().unwrap();
                manage::relocate(&mut backend, &paths, &to, true)?;
            }
            Some(Command::Rm { paths, recursive }) => manage::rm(&mut backend, &paths, recursive)?,
            Some(Command::Get { .. } | Command::Put { .. }) => unreachable!(),
            None => tui::run(&mut backend)?,
            Some(Command::Status) => unreachable!(),
        }
        Ok(())
    })();
    let _lock = account_lock;
    feedback::closing();
    let mut cleanup_error = None;
    if backend.authenticated && !backend.remembered {
        cancelled.store(false, Ordering::Relaxed);
        if let Err(e) = backend.revoke_on_exit() {
            if feedback::active() {
                cleanup_error = Some(e.context("Could not end the temporary login"));
            } else {
                feedback::note(format!("Session revocation failed: {}", e));
            }
        }
    }
    if let Err(e) = backend.finish() {
        if e.downcast_ref::<input::Cancelled>().is_some() {
            if result.is_ok() {
                return Err(e);
            }
        } else {
            if feedback::active() {
                cleanup_error = Some(e.context("Could not save the local index"));
            } else {
                feedback::note(format!("Could not save local index: {}", e));
            }
        }
    }
    if let Some(error) = cleanup_error {
        return Err(error);
    }
    result
}
fn main() {
    if let Err(e) = run(Args::parse()) {
        if !e.is::<feedback::DisplayedError>() {
            eprintln!("{}", error::message(&e));
        }
        std::process::exit(if e.downcast_ref::<input::Cancelled>().is_some() {
            130
        } else {
            1
        });
    }
}
