use anyhow::{Result, bail};

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
    // The libraries and the bindings come from the job that builds them. This
    // one holds the emulator and the measured group while the suite runs, and
    // building both ABIs again here spends that window on work already done.
    require_generated(process)?;
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

    // Named relative to the working directory the command is given, not to the
    // checkout: a relative program path is resolved against the child's own
    // directory, so `android/gradlew` from inside `android` looked for
    // `android/android/gradlew` and the lane failed to start after the
    // emulator had been up for twenty minutes.
    let mut gradle = process.command_in(
        if cfg!(windows) {
            "./gradlew.bat"
        } else {
            "./gradlew"
        },
        "android",
    );
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

/// Gradle is told to skip generating these, so a job that arrives without them
/// builds an archive with no library in it and fails on the device with
/// `UnsatisfiedLinkError`, minutes after the emulator came up. Say so here
/// instead, before anything boots.
fn require_generated(process: &Process) -> Result<()> {
    let generated = process.root().join("android/lib/build/generated");
    let library = generated.join("jniLibs/arm64-v8a/libkithara_ffi.so");
    if !library.is_file() {
        bail!("the Android build job did not leave {}", library.display());
    }
    let bindings = generated.join("uniffi/kotlin");
    if !bindings.is_dir() {
        bail!("the Android build job did not leave {}", bindings.display());
    }
    Ok(())
}
