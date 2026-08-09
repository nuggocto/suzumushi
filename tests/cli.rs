// SPDX-License-Identifier: Apache-2.0

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::symlink;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use rustix::fs::{CWD, Mode, mkfifoat};
use tempfile::TempDir;

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
fn bare_command_explains_how_to_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_suzumushi"))
        .output()
        .expect("the binary should start");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(stdout.contains("suzumushi [OPTIONS] [COMMAND]"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("diagnose"));
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
    fs::write(root.join("audio/lyrics/tagged.lrc"), b"[00:00] shared")
        .expect("write shared lyrics");
    fs::write(root.join("audio/library/cover.png"), b"candidate only")
        .expect("write artwork candidate");

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
        "shared lyrics: 1",
        "artwork candidates: 1",
        "Calm Artist - Night Song",
        "BrokenSymlink",
    ] {
        assert!(stdout.contains(expected), "missing {expected:?}:\n{stdout}");
    }
    assert!(
        !root.join(".suzumushi-root.lock").exists(),
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
