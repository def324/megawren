use std::process::Command;
#[test]
fn help_and_version_do_not_need_credentials() {
    for arg in ["--help", "--version", "get --help", "preview --help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_megawren"))
            .args(arg.split_whitespace())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
    }
}
#[test]
fn status_is_local_and_noninteractive() {
    let output = Command::new(env!("CARGO_BIN_EXE_megawren"))
        .arg("status")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(".megawren"));
    assert!(output.stderr.is_empty());
}
#[test]
fn malformed_command_does_not_start_authentication() {
    let output = Command::new(env!("CARGO_BIN_EXE_megawren"))
        .arg("put")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn release_help_explains_browser_and_collision_scope() {
    let exe = env!("CARGO_BIN_EXE_megawren");
    let help = |args: &[&str]| {
        String::from_utf8(Command::new(exe).args(args).output().unwrap().stdout).unwrap()
    };
    let overview = help(&["--help"]);
    assert!(overview.contains("open the file browser"));
    assert!(overview.contains("stdin, stdout, or stderr is redirected"));
    assert!(help(&["ls", "--help"]).contains("JSON array to stdout"));
    assert!(help(&["get", "--help"]).contains("existing local files"));
}

#[test]
fn redirected_commands_explain_setup_and_validate_inputs_before_unlocking() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().canonicalize().unwrap();
    std::fs::write(dir.join("first.txt"), b"one").unwrap();
    std::fs::write(dir.join("second.txt"), b"two").unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_megawren"))
            .current_dir(&dir)
            .env("HOME", &dir)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        String::from_utf8(output.stderr).unwrap()
    };
    assert!(
        run(&["--unlock-stdin", "ls", "--json"])
            .contains("No saved login. Run megawren login in a terminal.")
    );
    assert!(run(&["put", "first.txt", "second.txt", "/Photos"]).contains("No saved login."));
    assert!(run(&["get", "/a", "/b", "."]).contains("No saved login."));
    assert!(run(&["put", "first.txt", "missing.txt", "/Photos"]).contains("missing.txt"));
    assert!(run(&["get", "/a", "/b", "missing-dir"]).contains("missing-dir"));
    assert!(run(&["ls", "relative"]).contains("Remote paths must begin with /"));
    assert!(run(&["get"]).contains("remote path or MEGA link"));
    assert!(run(&["login"]).contains("terminal"));
    std::fs::create_dir(dir.join("nested")).unwrap();
    std::fs::write(dir.join("nested/first.txt"), b"different").unwrap();
    assert!(run(&["put", "first.txt", "nested/first.txt", "/"]).contains("same destination name"));
    let error = run(&[
        "get",
        "https://mega.nz/file/synthetic#secretpart",
        "/a",
        ".",
    ]);
    assert!(error.contains("public link"));
    assert!(!error.contains("secretpart"));
    let error = run(&["get", "/a", "https://mega.nz/file/synthetic#secretpart"]);
    assert!(error.contains("local directory"));
    assert!(!error.contains("secretpart"));
    let state = dir.join(".megawren");
    let session = state.join("session.enc");
    std::fs::write(&session, b"unused fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&session, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(run(&["ls"]).contains("--unlock-stdin"));
    let mut corrupt = vec![0; 84];
    corrupt[..12].copy_from_slice(b"MGWSESS\0\x02\0\x01\x01");
    std::fs::write(session, corrupt).unwrap();
    assert!(run(&["ls", "--json"]).contains("Cannot read saved login"));
}
