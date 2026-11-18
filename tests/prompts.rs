#![cfg(unix)]
#[path = "../src/input.rs"]
mod input;

use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn terminal_flags(fd: i32) -> libc::tcflag_t {
    let mut attrs = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(fd, attrs.as_mut_ptr()) }, 0);
    unsafe { attrs.assume_init() }.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG)
}
struct Pty {
    master: File,
    observer: File,
    child: Child,
    output: Vec<u8>,
}
impl Pty {
    fn new(mode: &str) -> Self {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        assert_ne!(terminal_flags(slave.as_raw_fd()) & libc::ECHO, 0);
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--ignored", "--exact", "prompt_child", "--nocapture"])
            .env("MEGAWREN_PROMPT_CASE", mode)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        Self {
            master,
            observer: slave,
            child,
            output: Vec::new(),
        }
    }
    fn until(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !String::from_utf8_lossy(&self.output).contains(needle) {
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for {needle}; output: {}",
                String::from_utf8_lossy(&self.output)
            );
            let mut poll = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 100) } > 0 {
                let mut bytes = [0u8; 4096];
                let n = self.master.read(&mut bytes).unwrap();
                assert!(n > 0, "PTY closed before prompt");
                self.output.extend_from_slice(&bytes[..n]);
            }
        }
    }
    fn type_bytes(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }
    fn signal(&self) {
        assert_eq!(unsafe { libc::kill(self.child.id() as _, libc::SIGINT) }, 0);
    }
    fn finish(&mut self, code: i32) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut poll = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 10) } > 0 && poll.revents & libc::POLLIN != 0 {
                let mut bytes = [0u8; 4096];
                if let Ok(n) = self.master.read(&mut bytes) {
                    self.output.extend_from_slice(&bytes[..n]);
                }
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(code));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Prompt did not exit after Ctrl-C; output: {}",
                String::from_utf8_lossy(&self.output)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The child asserts restoration before exit. macOS may invalidate the
        // slave's termios as the controlling session exits, so checking it here
        // can return EIO despite correct restoration.
        let output = String::from_utf8_lossy(&self.output);
        assert!(
            output.contains("Terminal restored") || output.contains("Input verified"),
            "No restoration confirmation: {output}"
        );
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // Close the PTY before reaping: a macOS session leader can be waiting
        // for terminal teardown while the parent still holds the master open.
        if let Ok(null) = File::open("/dev/null") {
            drop(std::mem::replace(&mut self.master, null));
        }
        if let Ok(null) = File::open("/dev/null") {
            drop(std::mem::replace(&mut self.observer, null));
        }
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn ctrl_c_cancels_visible_prompt_and_restores_terminal() {
    let mut p = Pty::new("plain");
    p.until("Email: ");
    p.type_bytes(&[3]);
    p.finish(130);
}
#[test]
fn ctrl_c_cancels_hidden_prompt_and_restores_terminal() {
    let mut p = Pty::new("hidden");
    p.until("Password: ");
    p.type_bytes(b"typed-but-not-submitted");
    p.type_bytes(&[3]);
    p.finish(130);
    assert!(!String::from_utf8_lossy(&p.output).contains("typed-but-not-submitted"));
}
#[test]
fn os_sigint_cancels_prompt_without_enter() {
    let mut p = Pty::new("hidden");
    p.until("Password: ");
    p.signal();
    p.finish(130);
}
#[test]
fn second_interrupt_forces_exit_after_prompt_cleanup() {
    let mut p = Pty::new("double");
    p.until("Password: ");
    p.type_bytes(&[3]);
    p.until("Simulating blocked cleanup");
    p.signal();
    p.finish(130);
}
#[test]
fn sequential_prompts_preserve_unicode_spaces_and_pasted_input() {
    let mut p = Pty::new("sequence");
    p.until("Email: ");
    // Both lines arrive together; switching from plain to hidden input must not
    // lose buffered bytes, trim spaces, or echo the second line.
    p.type_bytes("test@example.invalid\r  päss words  \r".as_bytes());
    p.until("Input verified");
    p.finish(0);
    assert!(!String::from_utf8_lossy(&p.output).contains("päss words"));
}

#[test]
fn bracketed_paste_preserves_hidden_input_without_echo() {
    let mut p = Pty::new("sequence");
    p.until("Email: ");
    p.type_bytes(b"test@example.invalid\r");
    p.until("Password: ");
    p.type_bytes("\x1b[200~  päss words  \x1b[201~\r".as_bytes());
    p.until("Input verified");
    p.finish(0);
    assert!(!String::from_utf8_lossy(&p.output).contains("päss words"));
}

#[test]
#[ignore = "spawned by PTY regression tests"]
fn prompt_child() {
    let mode = std::env::var("MEGAWREN_PROMPT_CASE").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    ctrlc::set_handler(move || input::interrupt(&signal)).unwrap();
    if mode.starts_with("pipe") {
        eprintln!("Pipe ready");
        let result = input::read_stdin_line(1024, &cancel);
        if mode == "pipe-cancel" {
            assert!(result.unwrap_err().is::<input::Cancelled>());
            std::process::exit(130);
        }
        assert!(result.unwrap().as_str() == "unlock fixture");
        return;
    }
    let before = terminal_flags(0);
    if mode == "sequence" {
        let email = input::read("Email: ", false, 1024, &cancel).unwrap();
        assert!(email.as_str() == "test@example.invalid");
        let password = input::read("Password: ", true, 1024, &cancel).unwrap();
        assert!(password.as_str() == "  päss words  ");
        assert_eq!(terminal_flags(0), before);
        eprintln!("Input verified");
        return;
    }
    let hidden = mode != "plain";
    let result = input::read(
        if hidden { "Password: " } else { "Email: " },
        hidden,
        1024,
        &cancel,
    );
    assert!(result.unwrap_err().is::<input::Cancelled>());
    assert_eq!(terminal_flags(0), before);
    eprintln!("Terminal restored");
    if mode == "double" {
        input::arm_exit(&cancel);
        eprintln!("Simulating blocked cleanup");
        loop {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(cancel.load(Ordering::Relaxed));
    std::process::exit(130);
}

fn pipe_child(mode: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "prompt_child", "--nocapture"])
        .env("MEGAWREN_PROMPT_CASE", mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
fn pipe_ready(child: &mut Child) {
    use std::io::BufRead;
    let stderr = child.stderr.take().unwrap();
    let (send, recv) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        std::io::BufReader::new(stderr)
            .read_line(&mut line)
            .unwrap();
        let _ = send.send(line);
    });
    let ready = recv.recv_timeout(Duration::from_secs(3));
    if ready.is_err() {
        let _ = child.kill();
    }
    assert!(ready.unwrap().contains("Pipe ready"));
}
fn pipe_finish(child: &mut Child, code: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(code));
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("Redirected input did not finish");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn redirected_input_returns_at_newline_without_waiting_for_eof() {
    let mut child = pipe_child("pipe-line");
    pipe_ready(&mut child);
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"unlock fixture\n")
        .unwrap();
    pipe_finish(&mut child, 0);
}
#[test]
fn sigint_cancels_redirected_input_while_writer_stays_open() {
    let mut child = pipe_child("pipe-cancel");
    pipe_ready(&mut child);
    assert_eq!(unsafe { libc::kill(child.id() as _, libc::SIGINT) }, 0);
    pipe_finish(&mut child, 130);
}
