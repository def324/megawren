use anyhow::{Result, ensure};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled},
};
use std::{
    fmt,
    io::{self, IsTerminal, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct Cancelled;
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
pub fn arm_exit(cancel: &AtomicBool) {
    INTERRUPTED.store(true, Ordering::Relaxed);
    cancel.store(true, Ordering::Relaxed);
}

pub fn interrupt(cancel: &AtomicBool) {
    if INTERRUPTED.swap(true, Ordering::Relaxed) {
        force_exit();
    }
    cancel.store(true, Ordering::Relaxed);
}
impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cancelled")
    }
}
impl std::error::Error for Cancelled {}
pub fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }
    Ok(())
}

struct RawGuard {
    restore: bool,
}
impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), event::DisableBracketedPaste);
        if self.restore {
            let _ = disable_raw_mode();
        }
    }
}

pub fn can_prompt() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Use the existing terminal event backend so both keypresses and OS signals
/// can cancel a prompt. Secret input is never rendered or trimmed.
pub fn read(
    label: &str,
    hidden: bool,
    max: usize,
    cancel: &AtomicBool,
) -> Result<Zeroizing<String>> {
    ensure!(
        can_prompt(),
        "Interactive input requires a terminal for stdin and stderr"
    );
    check(cancel)?;
    let was_raw = is_raw_mode_enabled()?;
    enable_raw_mode()?;
    let _guard = RawGuard { restore: !was_raw };
    execute!(io::stderr(), event::EnableBracketedPaste)?;
    let mut stderr = io::stderr();
    write!(stderr, "{label}")?;
    stderr.flush()?;
    let mut text = Zeroizing::new(String::new());
    loop {
        if let Err(error) = check(cancel) {
            write!(stderr, "\r\n")?;
            return Err(error);
        }
        let ready = match event::poll(Duration::from_millis(50)) {
            Ok(ready) => ready,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if !ready {
            continue;
        }
        let input = match event::read() {
            Ok(input) => input,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if let Event::Paste(pasted) = input {
            let pasted = Zeroizing::new(pasted);
            ensure!(
                !pasted.chars().any(char::is_control),
                "Pasted input contains control characters"
            );
            ensure!(text.len() + pasted.len() <= max, "Input too long");
            text.push_str(&pasted);
            if !hidden {
                write!(stderr, "{}", *pasted)?;
                stderr.flush()?;
            }
            continue;
        }
        let Event::Key(key) = input else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            interrupt(cancel);
            write!(stderr, "\r\n")?;
            return Err(Cancelled.into());
        }
        match key.code {
            KeyCode::Enter => {
                write!(stderr, "\r\n")?;
                return Ok(text);
            }
            KeyCode::Char('d')
                if key.modifiers.contains(KeyModifiers::CONTROL) && text.is_empty() =>
            {
                write!(stderr, "\r\n")?;
                return Err(Cancelled.into());
            }
            KeyCode::Backspace => {
                if text.pop().is_some() && !hidden {
                    write!(stderr, "\u{8} \u{8}")?;
                    stderr.flush()?;
                }
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                ensure!(text.len() + c.len_utf8() <= max, "Input too long");
                text.push(c);
                if !hidden {
                    write!(stderr, "{c}")?;
                    stderr.flush()?;
                }
            }
            _ => (),
        }
    }
}

pub fn force_exit() -> ! {
    let _ = disable_raw_mode();
    if io::stdout().is_terminal() {
        let _ = execute!(
            io::stdout(),
            event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    } else if io::stderr().is_terminal() {
        let _ = execute!(
            io::stderr(),
            event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
    eprintln!("Interrupted; private working files may remain.");
    std::process::exit(130)
}

/// Read exactly one bounded line, without waiting for the writer to close its
/// pipe. Polling keeps redirected unlock input cancellable too.
#[cfg(unix)]
pub fn read_stdin_line(max: usize, cancel: &AtomicBool) -> Result<Zeroizing<String>> {
    read_fd_line(libc::STDIN_FILENO, max, cancel)
}
#[cfg(unix)]
pub fn read_fd_line(fd: i32, max: usize, cancel: &AtomicBool) -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        check(cancel)?;
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if ready == 0 {
            continue;
        }
        let mut byte = 0u8;
        let n = unsafe { libc::read(fd, std::ptr::addr_of_mut!(byte).cast(), 1) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if n == 0 || byte == b'\n' {
            break;
        }
        ensure!(bytes.len() < max, "Unlock input too long");
        bytes.push(byte);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Zeroizing::new(std::str::from_utf8(&bytes)?.to_owned()))
}
#[cfg(not(unix))]
pub fn read_stdin_line(_max: usize, _cancel: &AtomicBool) -> Result<Zeroizing<String>> {
    anyhow::bail!("Redirected unlock input is not supported on this platform yet")
}
