//! Test-only helpers shared by the `ci` suites.

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
