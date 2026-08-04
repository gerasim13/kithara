use anyhow::Result;

use crate::ci::{
    config::CiConfig,
    process::{Process, require_os},
};

pub(crate) fn build(process: &Process) -> Result<()> {
    require_os("macos", "Android")?;
    process.require_tools(&["cargo", "java", "just", "sccache"])?;
    native_build(process)
}

/// The emulator, the connected suite, and the shutdown are one job: the tests
/// only mean anything while the device this job booted is still alive, and a
/// device left running would poison the next Android job on the host.
pub(crate) fn test(process: &Process, config: &CiConfig) -> Result<()> {
    require_os("macos", "Android emulator")?;
    process.require_tools(&["adb", "cargo", "emulator", "java", "just", "sccache"])?;
    native_build(process)?;
    process.run(
        "just",
        &[
            "platform",
            "android",
            "run",
            "--avd",
            &config.pins.android_avd,
            "--skip-build",
        ],
        "Android emulator launch",
    )?;

    let mut gradle = process.command(if cfg!(windows) {
        "android/gradlew.bat"
    } else {
        "android/gradlew"
    });
    gradle.args([
        ":lib:connectedDebugAndroidTest",
        "-x",
        "generateKitharaFfi",
        "--no-daemon",
    ]);
    let tests = process.run_command(&mut gradle, "Android connected tests");
    let cleanup = process.run("adb", &["emu", "kill"], "Android emulator shutdown");
    match (tests, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), cleanup) => cleanup,
    }
}

fn native_build(process: &Process) -> Result<()> {
    process.run(
        "just",
        &["platform", "android", "build"],
        "Android native build",
    )
}
