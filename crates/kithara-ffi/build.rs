use std::{env, process::Command};

/// The commit web release packaging builds from. Only that packaging sets it,
/// so a test build embeds fixed values and reruns on no commit.
const REVISION_ENV: &str = "KITHARA_BUILD_REVISION";

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

/// Only the web bindings expose build metadata, and only a release names the
/// commit it describes; every other build embeds the same fixed values.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_ARCH").ok().as_deref() != Some("wasm32") {
        return;
    }
    println!("cargo::rerun-if-env-changed={REVISION_ENV}");
    let Ok(revision) = env::var(REVISION_ENV) else {
        println!("cargo::rustc-env=BUILD_GIT_HASH=unknown");
        println!("cargo::rustc-env=BUILD_TIMESTAMP=0000-0000");
        return;
    };
    let (Some(hash), Some(timestamp)) = (
        git_output(&["rev-parse", "--short=8", &revision]),
        git_output(&[
            "show",
            "-s",
            "--format=%cd",
            "--date=format:%m%d-%H%M%S",
            &revision,
        ]),
    ) else {
        println!("cargo::error=git cannot describe {REVISION_ENV}={revision}");
        return;
    };
    println!("cargo::rustc-env=BUILD_GIT_HASH={hash}");
    println!("cargo::rustc-env=BUILD_TIMESTAMP={timestamp}");
}
