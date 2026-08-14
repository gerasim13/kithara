use anyhow::{Result, bail};

use crate::ci::{config::CiConfig, process::Process};

pub(crate) fn build(process: &Process) -> Result<()> {
    process.require_os("macos", "Android")?;
    process.require_tools(&["cargo", "java", "just", "sccache"])?;
    native_build(process)
}

/// The emulator, the connected suite, and the shutdown are one job: the tests
/// only mean anything while the device this job booted is still alive, and a
/// device left running would poison the next Android job on the host.
pub(crate) fn test(process: &Process, config: &CiConfig) -> Result<()> {
    // The emulator runs wherever the machine's own architecture matches the
    // guest's; on a host that has to emulate a foreign one it is merely slow.
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
            "test",
            "--avd",
            &config.pins.android_avd,
            "--skip-build",
        ],
        "Android connected tests",
    )
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
