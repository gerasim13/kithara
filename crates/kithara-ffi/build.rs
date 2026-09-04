use std::process::Command;

fn main() {
    // Git short hash
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=BUILD_GIT_HASH={}", git_hash.trim());

    // Build timestamp (compact: MMDD-HHMM)
    let ts = Command::new("date")
        .args(["+%m%d-%H%M%S"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "0000-0000".into());
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", ts.trim());

    // Rebuild when git HEAD changes or any source file changes
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--git-path", "HEAD"])
        .output()
        && output.status.success()
        && let Ok(path) = String::from_utf8(output.stdout)
    {
        println!("cargo:rerun-if-changed={}", path.trim());
    }
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=build.rs");
}
