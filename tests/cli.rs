// SPDX-License-Identifier: Apache-2.0

use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use rustix::fs::{CWD, Mode, OFlags, mkfifoat};
use rustix::process::{Signal, kill_process};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Pid, Winsize, tcsetwinsize};
use tempfile::TempDir;

const FOCUSED_PLAYER: &[u8] = b"\x1b[7m\x1b[1m\x1b[38;5;6;49mPlayer";

#[test]
fn help_succeeds_and_names_the_canonical_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("--help")
        .output()
        .expect("the suzumushi binary should start");

    assert!(
        output.status.success(),
        "--help failed with status {}",
        output.status
    );

    let stdout = String::from_utf8(output.stdout).expect("help output should be valid UTF-8");

    assert!(
        stdout.contains("Usage: suzumushi"),
        "help did not name the canonical command:\n{stdout}"
    );
    assert!(
        stdout.contains("--help"),
        "help did not describe its help option:\n{stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "--help wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bare_command_requires_a_selected_root() {
    let temp = TempDir::new().expect("temporary directory");
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .current_dir(temp.path())
        .env_remove("SUZUMUSHI_ROOT")
        .output()
        .expect("the binary should start");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 error");
    assert!(stderr.contains("suzumushi init ./suzumushi"), "{stderr}");
}

#[test]
fn terminal_session_restores_the_pty_and_holds_both_leases() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    initialize_root(&root);
    tune_terminal_fixture(&root);
    seed_old_log_archives(&root);
    let runtime = temp.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        .expect("make runtime directory private");

    let (mut master, slave) = open_test_pty(80, 24);
    let mut child = terminal_command(&root, &runtime)
        .stdin(Stdio::from(duplicate_file(&slave)))
        .stdout(Stdio::from(duplicate_file(&slave)))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("start terminal session");
    let mut transcript = read_pty_until(&mut master, &mut child, b"Library");

    master.write_all(b"\t").expect("send focus key");
    transcript.extend(read_pty_until(&mut master, &mut child, FOCUSED_PLAYER));

    resize_terminal(&master, &child, 40, 10);
    transcript.extend(read_pty_until(&mut master, &mut child, b"needs"));

    let second = terminal_command(&root, &runtime)
        .output()
        .expect("run competing terminal session");
    assert!(!second.status.success());
    let conflict = String::from_utf8(second.stderr).expect("UTF-8 lock error");
    assert!(
        conflict.contains("active TUI lease is already held"),
        "{conflict}"
    );

    master.write_all(b"q").expect("send quit key");
    transcript.extend(read_pty_to_exit(&mut master, &mut child));
    assert!(byte_contains(&transcript, b"\x1b[?1049h"));
    assert!(byte_contains(&transcript, b"\x1b[?1049l"));
    assert!(!byte_contains(&transcript, b"terminal session starting"));

    let current = root.join("logs/suzumushi.log");
    let archive = root.join("logs/suzumushi.log.1");
    assert!(current.is_file());
    assert!(
        archive.is_file(),
        "small fixture limit should rotate the log"
    );
    assert_eq!(
        fs::metadata(&current)
            .expect("current log metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(
        byte_contains(
            &fs::read(&current).expect("read current log"),
            b"[truncated]"
        ) || byte_contains(
            &fs::read(&archive).expect("read archived log"),
            b"[truncated]"
        ),
        "the installed logging pipeline should mark truncated records"
    );
    for index in 2..=4 {
        assert!(
            !root.join(format!("logs/suzumushi.log.{index}")).exists(),
            "lower retention should prune archive {index}"
        );
    }
    assert_eq!(
        fs::metadata(&archive)
            .expect("archive log metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    suzumushi::locks::ActiveTuiLease::acquire_in(Some(&runtime))
        .expect("active lease is released after quit");
    suzumushi::locks::RootWriterLease::acquire(&root).expect("root lease is released after quit");
    suzumushi::init::initialize(&root)
        .expect("the persistent root lock remains an owned initialization entry");
}

#[test]
fn oversized_terminal_resize_is_refused_before_buffer_growth() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    initialize_root(&root);
    let runtime = temp.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        .expect("make runtime directory private");

    let (mut master, slave) = open_test_pty(80, 24);
    let mut child = terminal_command(&root, &runtime)
        .stdin(Stdio::from(duplicate_file(&slave)))
        .stdout(Stdio::from(duplicate_file(&slave)))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("start terminal session");
    let mut transcript = read_pty_until(&mut master, &mut child, b"Library");
    master
        .write_all(b"\t")
        .expect("prime terminal event delivery");
    transcript.extend(read_pty_until(&mut master, &mut child, FOCUSED_PLAYER));

    let pair_bytes = std::mem::size_of::<ratatui::buffer::Cell>() * 2;
    let width = 512_usize;
    let height = (8 * 1_048_576 / pair_bytes / width) + 1;
    let height = u16::try_from(height).expect("fixture dimensions fit the PTY");
    resize_terminal(
        &master,
        &child,
        u16::try_from(width).expect("fixture width"),
        height,
    );
    let (tail, status) = read_pty_to_status(&mut master, &mut child);
    transcript.extend(tail);

    assert!(!status.success(), "oversized terminal must fail closed");
    assert!(byte_contains(
        &transcript,
        b"above the 8388608-byte UI reservation"
    ));
    assert!(byte_contains(&transcript, b"\x1b[?1049l"));
    suzumushi::locks::ActiveTuiLease::acquire_in(Some(&runtime))
        .expect("active lease is released after resize refusal");
    suzumushi::locks::RootWriterLease::acquire(&root)
        .expect("root lease is released after resize refusal");
}

#[test]
fn background_log_failure_is_reported_after_terminal_restoration() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    initialize_root(&root);
    tune_terminal_fixture(&root);
    let runtime = temp.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        .expect("make runtime directory private");

    let (mut master, slave) = open_test_pty(80, 24);
    let mut child = terminal_command(&root, &runtime)
        .stdin(Stdio::from(duplicate_file(&slave)))
        .stdout(Stdio::from(duplicate_file(&slave)))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("start terminal session");
    let mut transcript = read_pty_until(&mut master, &mut child, b"Library");

    let current = root.join("logs/suzumushi.log");
    wait_for_nonempty_file(&current, &mut child);
    fs::rename(&current, root.join("logs/suzumushi.log.held"))
        .expect("replace current log identity");
    fs::write(&current, b"replacement").expect("create replacement log");
    fs::set_permissions(&current, fs::Permissions::from_mode(0o600))
        .expect("make replacement log private");

    master.write_all(b"q").expect("send quit key");
    let (tail, status) = read_pty_to_status(&mut master, &mut child);
    transcript.extend(tail);
    assert!(!status.success(), "writer failure must fail the command");
    let restore = byte_position(&transcript, b"\x1b[?1049l").expect("alternate-screen exit");
    let diagnostic = byte_position(&transcript, b"background log writer failed")
        .expect("visible writer failure");
    assert!(
        restore < diagnostic,
        "diagnostic must follow terminal restoration"
    );

    suzumushi::locks::ActiveTuiLease::acquire_in(Some(&runtime))
        .expect("active lease is released after writer failure");
    suzumushi::locks::RootWriterLease::acquire(&root)
        .expect("root lease is released after writer failure");
}

#[test]
fn fifo_log_entry_never_blocks_terminal_startup() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    initialize_root(&root);
    let fifo = root.join("logs/suzumushi.log");
    mkfifoat(CWD, &fifo, Mode::from_raw_mode(0o600)).expect("create log FIFO");
    let _reader = rustix::fs::open(
        &fifo,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("hold FIFO reader");
    let runtime = temp.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        .expect("make runtime directory private");

    let output = output_with_timeout(&mut terminal_command(&root, &runtime));
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 logging error");
    assert!(stderr.contains("single-link regular file"), "{stderr}");
    suzumushi::locks::ActiveTuiLease::acquire_in(Some(&runtime))
        .expect("active lease is released after startup failure");
    suzumushi::locks::RootWriterLease::acquire(&root)
        .expect("root lease is released after startup failure");
}

#[test]
fn terminal_startup_reserves_its_open_file_peak_before_opening() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    initialize_root(&root);
    let config_path = root.join("config.toml");
    let config = fs::read_to_string(&config_path)
        .expect("read fixture config")
        .replacen("max_open_files = 64", "max_open_files = 4", 1);
    fs::write(&config_path, config).expect("lower open-file limit");
    let runtime = temp.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        .expect("make runtime directory private");

    let output = terminal_command(&root, &runtime)
        .output()
        .expect("run terminal startup preflight");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 resource error");
    assert!(stderr.contains("runtime.max_open_files"), "{stderr}");
    assert!(stderr.contains("at least 5"), "{stderr}");
    assert!(
        !runtime.join("suzumushi").exists(),
        "preflight must run before the active lease creates storage"
    );
    assert!(
        !root.join("logs/suzumushi.log").exists(),
        "preflight must run before logging opens a file"
    );
}

#[test]
fn invalid_arguments_have_one_clean_error_prefix() {
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .output()
        .expect("the binary should reject incomplete arguments");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("argument error should be valid UTF-8");
    assert!(
        stderr.starts_with("error: one or more required arguments were not provided"),
        "{stderr:?}"
    );
    assert_eq!(stderr.matches("error:").count(), 1, "{stderr:?}");
    assert!(!stderr.contains("invalid config"), "{stderr:?}");
    assert!(!stderr.contains(r"\x0A"), "{stderr:?}");
    assert!(stderr.ends_with('\n'));
}

#[test]
fn init_and_diagnose_work_through_the_real_executable() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let init = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .args(["init", root.to_str().expect("UTF-8 fixture path")])
        .output()
        .expect("run init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let root_lock = root.join(".suzumushi-root.lock");
    let lock_identity = fs::read(&root_lock).expect("initialization leaves the owned root lock");

    fs::write(root.join("audio/library/tagged.mp3"), id3_fixture()).expect("write tagged fixture");
    let playlist = root.join("audio/playlists/links");
    fs::create_dir(&playlist).expect("create playlist");
    symlink(
        root.join("audio/library/tagged.mp3"),
        playlist.join("tagged.mp3"),
    )
    .expect("create file symlink");
    symlink(
        root.join("audio/library/missing.mp3"),
        playlist.join("broken.mp3"),
    )
    .expect("create broken symlink");

    let diagnose = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .args([
            "diagnose",
            "--root",
            root.to_str().expect("UTF-8 fixture path"),
        ])
        .output()
        .expect("run diagnose");
    assert!(
        diagnose.status.success(),
        "{}",
        String::from_utf8_lossy(&diagnose.stderr)
    );
    let stdout = String::from_utf8(diagnose.stdout).expect("diagnose output is UTF-8");
    for expected in [
        "assets: 1",
        "tracks: 2",
        "Calm Artist - Night Song",
        "BrokenSymlink",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
    assert_eq!(
        fs::read(&root_lock).expect("root lock remains after diagnostics"),
        lock_identity,
        "diagnose must not claim the writer lease"
    );
}

#[test]
fn missing_root_fails_clearly_without_scanning_home() {
    let temp = TempDir::new().expect("temporary directory");
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("diagnose")
        .current_dir(temp.path())
        .env_remove("SUZUMUSHI_ROOT")
        .output()
        .expect("run diagnose");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 error");
    assert!(stderr.contains("suzumushi init ./suzumushi"));
    assert!(!temp.path().join("Music").exists());
}

#[test]
fn diagnostics_never_emit_raw_terminal_escape_bytes() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let init = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .arg(&root)
        .output()
        .expect("run init");
    assert!(init.status.success());
    fs::write(root.join("audio/library/bad\u{1b}[31m.mp3"), b"not media")
        .expect("write hostile filename fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("diagnose")
        .arg("--root")
        .arg(&root)
        .output()
        .expect("run diagnose");
    assert!(output.status.success());
    assert!(!output.stdout.contains(&0x1b));
    assert!(!output.stderr.contains(&0x1b));
}

#[test]
fn native_linux_paths_do_not_require_utf8() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join(OsString::from_vec(b"root-\xff".to_vec()));
    let init = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .arg(&root)
        .output()
        .expect("run init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let diagnose = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("diagnose")
        .arg("--root")
        .arg(&root)
        .output()
        .expect("run diagnose");
    assert!(
        diagnose.status.success(),
        "{}",
        String::from_utf8_lossy(&diagnose.stderr)
    );
    assert!(String::from_utf8(diagnose.stdout).is_ok());
}

#[test]
fn fifo_media_targets_never_wait_for_a_writer() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let init = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .arg(&root)
        .output()
        .expect("run init");
    assert!(init.status.success());

    let fifo = temp.path().join("media.fifo");
    mkfifoat(CWD, &fifo, Mode::from_raw_mode(0o600)).expect("create media FIFO");
    symlink(&fifo, root.join("audio/library/song.mp3")).expect("create FIFO symlink");

    let output = output_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_suzumushi"))
            .arg("diagnose")
            .arg("--root")
            .arg(&root),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 diagnostics");
    assert!(stdout.contains("NonRegularTarget"), "{stdout}");
}

#[test]
fn fifo_config_never_waits_for_a_writer() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let init = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .arg(&root)
        .output()
        .expect("run init");
    assert!(init.status.success());

    fs::remove_file(root.join("config.toml")).expect("remove regular config");
    mkfifoat(CWD, root.join("config.toml"), Mode::from_raw_mode(0o600))
        .expect("create config FIFO");

    let output = output_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_suzumushi"))
            .arg("diagnose")
            .arg("--root")
            .arg(&root),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 error");
    assert!(stderr.contains("single-link regular file"), "{stderr}");
}

#[test]
fn fifo_readme_never_blocks_repeated_initialization() {
    let temp = TempDir::new().expect("temporary directory");
    let root = temp.path().join("root");
    let init = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .arg(&root)
        .output()
        .expect("run init");
    assert!(init.status.success());

    let readme = root.join("audio/playlists/demo/README.txt");
    fs::remove_file(&readme).expect("remove regular README");
    mkfifoat(CWD, &readme, Mode::from_raw_mode(0o644)).expect("create README FIFO");

    let output = output_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_suzumushi"))
            .arg("init")
            .arg(&root),
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 error");
    assert!(stderr.contains("not a regular file"), "{stderr}");
    assert!(
        stderr.contains(readme.to_string_lossy().as_ref()),
        "{stderr}"
    );
}

fn initialize_root(root: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .arg("init")
        .arg(root)
        .output()
        .expect("initialize terminal fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tune_terminal_fixture(root: &std::path::Path) {
    let path = root.join("config.toml");
    let mut config = fs::read_to_string(&path).expect("read fixture config");
    for (old, new) in [
        ("max_file_bytes = 10485760", "max_file_bytes = 24"),
        ("max_record_bytes = 16384", "max_record_bytes = 24"),
        ("queue_capacity = 512", "queue_capacity = 8"),
        ("queue_max_bytes = 8388608", "queue_max_bytes = 512"),
        ("\nmax_files = 5\n", "\nmax_files = 2\n"),
    ] {
        assert_eq!(config.matches(old).count(), 1, "fixture key {old}");
        config = config.replacen(old, new, 1);
    }
    fs::write(path, config).expect("write fixture config");
}

fn seed_old_log_archives(root: &std::path::Path) {
    for index in 1..=4 {
        let archive = root.join(format!("logs/suzumushi.log.{index}"));
        fs::write(&archive, b"old archive").expect("seed old log archive");
        fs::set_permissions(&archive, fs::Permissions::from_mode(0o600))
            .expect("make old archive private");
    }
}

fn terminal_command(root: &std::path::Path, runtime: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_suzumushi"));
    command
        .arg("--root")
        .arg(root)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("TERM", "xterm-256color")
        .env_remove("NO_COLOR");
    command
}

fn open_test_pty(width: u16, height: u16) -> (File, File) {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .expect("open pseudoterminal");
    grantpt(&master).expect("grant pseudoterminal");
    unlockpt(&master).expect("unlock pseudoterminal");
    let name = ptsname(&master, Vec::new()).expect("resolve pseudoterminal slave");
    let path = std::path::Path::new(OsStr::from_bytes(name.to_bytes()));
    let slave = rustix::fs::open(
        path,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .expect("open pseudoterminal slave");
    tcsetwinsize(
        &slave,
        Winsize {
            ws_row: height,
            ws_col: width,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .expect("set pseudoterminal size");
    rustix::fs::fcntl_setfl(&master, OFlags::NONBLOCK).expect("make master nonblocking");
    (File::from(master), File::from(slave))
}

fn resize_terminal(master: &File, child: &Child, width: u16, height: u16) {
    tcsetwinsize(
        master,
        Winsize {
            ws_row: height,
            ws_col: width,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .expect("resize pseudoterminal");
    let raw_pid = i32::try_from(child.id()).expect("child PID fits i32");
    let pid = Pid::from_raw(raw_pid).expect("child PID is positive");
    kill_process(pid, Signal::WINCH).expect("deliver terminal resize");
}

fn duplicate_file(file: &File) -> File {
    File::from(rustix::io::dup(file).expect("duplicate pseudoterminal descriptor"))
}

fn read_pty_until(master: &mut File, child: &mut Child, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        read_available(master, &mut bytes);
        if byte_contains(&bytes, needle) {
            return bytes;
        }
        if let Some(status) = child.try_wait().expect("inspect terminal child") {
            panic!(
                "terminal exited before UI with {status}: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        std::thread::yield_now();
    }
    child.kill().expect("kill timed-out terminal");
    child.wait().expect("reap timed-out terminal");
    panic!(
        "terminal did not render {:?}: {}",
        String::from_utf8_lossy(needle),
        String::from_utf8_lossy(&bytes)
    );
}

fn read_pty_to_exit(master: &mut File, child: &mut Child) -> Vec<u8> {
    let (bytes, status) = read_pty_to_status(master, child);
    assert!(status.success(), "terminal exited with {status}");
    bytes
}

fn read_pty_to_status(master: &mut File, child: &mut Child) -> (Vec<u8>, ExitStatus) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    loop {
        read_available(master, &mut bytes);
        match child.try_wait() {
            Ok(Some(status)) => {
                read_available(master, &mut bytes);
                return (bytes, status);
            }
            Ok(None) if Instant::now() < deadline => std::thread::yield_now(),
            Ok(None) => {
                child.kill().expect("kill timed-out terminal");
                child.wait().expect("reap timed-out terminal");
                panic!("terminal did not quit: {}", String::from_utf8_lossy(&bytes));
            }
            Err(error) => panic!("cannot inspect terminal child: {error}"),
        }
    }
}

fn wait_for_nonempty_file(path: &std::path::Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0) {
            return;
        }
        if let Some(status) = child.try_wait().expect("inspect terminal child") {
            panic!("terminal exited before writing its startup log: {status}");
        }
        std::thread::yield_now();
    }
    child.kill().expect("kill terminal without startup log");
    child.wait().expect("reap terminal without startup log");
    panic!("terminal did not write its startup log");
}

fn read_available(master: &mut File, output: &mut Vec<u8>) {
    let mut buffer = [0_u8; 4_096];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
            // Linux PTYs return EIO after their final slave closes.
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => {
                return;
            }
            Err(error) => panic!("read pseudoterminal: {error}"),
        }
    }
}

fn byte_contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn byte_position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn output_with_timeout(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().expect("collect command output"),
            Ok(None) if Instant::now() < deadline => std::thread::yield_now(),
            Ok(None) => {
                child.kill().expect("kill timed-out command");
                let output = child.wait_with_output().expect("reap timed-out command");
                panic!(
                    "command exceeded two seconds\nstdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(error) => {
                child.kill().expect("kill command after wait failure");
                child.wait().expect("reap command after wait failure");
                panic!("cannot inspect command status: {error}");
            }
        }
    }
}

fn id3_fixture() -> Vec<u8> {
    fn frame(id: [u8; 4], value: &str) -> Vec<u8> {
        let mut body = vec![3];
        body.extend_from_slice(value.as_bytes());
        let mut frame = id.to_vec();
        frame.extend_from_slice(
            &u32::try_from(body.len())
                .expect("small fixture")
                .to_be_bytes(),
        );
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&body);
        frame
    }
    let mut frames = Vec::new();
    frames.extend(frame(*b"TIT2", "Night Song"));
    frames.extend(frame(*b"TPE1", "Calm Artist"));
    frames.extend(frame(*b"TPE2", "Album Artist"));
    frames.extend(frame(*b"TALB", "Quiet Album"));
    let size = u32::try_from(frames.len()).expect("small fixture");
    let syncsafe = [
        ((size >> 21) & 0x7f) as u8,
        ((size >> 14) & 0x7f) as u8,
        ((size >> 7) & 0x7f) as u8,
        (size & 0x7f) as u8,
    ];
    let mut file = b"ID3\x03\x00\x00".to_vec();
    file.extend_from_slice(&syncsafe);
    file.extend(frames);
    // One complete MPEG-1 Layer III frame lets content probing establish the container.
    file.extend_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
    file.resize(file.len() + 413, 0);
    file
}
