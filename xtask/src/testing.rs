//! Test doubles for the external programs `xtask` spawns.

#[cfg(unix)]
use std::process::Command;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Put the workspace's `fake-tool` where the code under test will look.
///
/// A unit test cannot ask Cargo where a binary target landed, so it is located
/// relative to this test binary: `target/<profile>/deps/<test>` sits one level
/// below the directory Cargo writes binaries into. `--bins` builds the binary
/// as a harness rather than as itself, which is why the message names the
/// build command.
pub(crate) fn install_double(bin: &Path, role: &str) -> PathBuf {
    let source = std::env::current_exe()
        .expect("current test executable")
        .parent()
        .and_then(Path::parent)
        .expect("the test binary lives under the profile directory")
        .join(format!("fake-tool{}", std::env::consts::EXE_SUFFIX));
    assert!(
        source.is_file(),
        "build the fake tool first: cargo build -p xtask --bin fake-tool ({})",
        source.display()
    );
    fs::create_dir_all(bin).expect("create the tool directory");
    let destination = bin.join(format!("{role}{}", std::env::consts::EXE_SUFFIX));
    publish(&source, &destination);
    destination
}

/// A copy is written, and a test thread that forks while the copy is open for
/// writing hands that descriptor to its child; executing the copy then fails
/// with "Text file busy" until the child execs. A link writes nothing.
#[cfg(unix)]
fn publish(source: &Path, destination: &Path) {
    std::os::unix::fs::symlink(source, destination).expect("publish the fake tool");
}

#[cfg(not(unix))]
fn publish(source: &Path, destination: &Path) {
    fs::copy(source, destination).expect("publish the fake tool");
}

/// Install `body` as an executable script at `path`, for a double whose
/// behaviour the environment of the code under test cannot select.
///
/// Writing it from this process would open the "Text file busy" window [`publish`]
/// avoids, and a rename cannot close it: the leaked descriptor names the inode.
/// A child shell writes it instead and is reaped before this returns, so no
/// process that could still hold a writable descriptor for it remains.
#[cfg(unix)]
pub(crate) fn install_script(path: &Path, body: &str) {
    let status = Command::new("sh")
        .args([
            "-c",
            r#"printf '%s' "$1" > "$2" && chmod 755 "$2""#,
            "sh",
            body,
        ])
        .arg(path)
        .status()
        .expect("spawn the script writer");
    assert!(status.success(), "write the script {}", path.display());
}

#[cfg(unix)]
#[test]
fn executable_alias_preserves_the_tool_role() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let executable = install_double(directory.path(), "launchctl");
    assert!(executable.is_file());
    let status = Command::new(executable)
        .arg("bootout")
        .env("KITHARA_TEST_RULES", "launchctl:bootout:*=7,*:*:*=9")
        .status()
        .expect("execute the published tool alias");
    assert_eq!(status.code(), Some(7));
}

/// Only Linux refuses to execute a file some process can still write to, so
/// only Linux can tell this writer from a direct one.
#[cfg(target_os = "linux")]
#[test]
fn an_installed_script_runs_while_other_threads_fork() {
    use std::{
        sync::mpsc::{self, TryRecvError},
        thread,
    };

    let directory = tempfile::tempdir().expect("temporary directory");
    let (running, stopped) = mpsc::channel::<()>();
    thread::scope(move |scope| {
        scope.spawn(move || {
            while matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                Command::new("true").status().expect("fork a bystander");
            }
        });
        for index in 0..200 {
            let script = directory.path().join(format!("script-{index}"));
            install_script(&script, "#!/bin/sh\nexit 7\n");
            let status = Command::new(&script)
                .status()
                .expect("execute the installed script");
            assert_eq!(status.code(), Some(7));
        }
        drop(running);
    });
}
