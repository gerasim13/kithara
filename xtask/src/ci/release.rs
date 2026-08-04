use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use kithara_devtools::Ctx;
use sha2::{Digest, Sha256};

use super::process::{Process, require_os};
use crate::{
    config::{KitharaExt, ReleaseConfig},
    publish, release,
};

/// The retained framework, the documentation archive, and the WASM bundle are
/// built by three jobs. They share nothing but the checkout, so a failure in
/// one is retried without rebuilding the other two, and the release pipeline
/// shows which artifact is missing by name.
pub(crate) fn xcframework(
    process: &Process,
    ctx: &Ctx,
    ext: &KitharaExt,
    temp: &Path,
) -> Result<()> {
    require_os("macos", "Apple release")?;
    let version = required_env("KITHARA_RELEASE_VERSION")?;
    let manifest = fs::read_to_string(ctx.root.join(&ext.release.manifest))
        .with_context(|| format!("reading {}", ext.release.manifest))?;
    let manifest_version = manifest_field(&manifest, "version")?;
    let manifest_checksum = manifest_field(&manifest, "checksum")?;
    if manifest_version != version {
        bail!(
            "{} version is {manifest_version}, requested {version}",
            ext.release.manifest
        );
    }

    process.run(
        "just",
        &["platform", "apple", "release"],
        "Apple release artifacts",
    )?;
    for name in [&ext.release.asset, &ext.release.single_asset] {
        if name.is_empty() {
            continue;
        }
        let source = temp.join(name);
        copy_required(&source, &ctx.root.join(name))?;
    }

    let primary = ctx.root.join(&ext.release.asset);
    let actual_checksum = swift_checksum(process, &primary)?;
    if actual_checksum != manifest_checksum {
        bail!(
            "{} checksum does not match the retained XCFramework: expected \
             {manifest_checksum}, found {actual_checksum}",
            ext.release.manifest
        );
    }

    for name in [&ext.release.asset, &ext.release.single_asset] {
        if !name.is_empty() {
            write_checksum(&ctx.root.join(name))?;
        }
    }
    Ok(())
}

pub(crate) fn docs(process: &Process, ctx: &Ctx, ext: &KitharaExt) -> Result<()> {
    require_os("macos", "Apple documentation release")?;
    // The documentation is generated against the local Swift package, and the
    // package resolves its binary target from the debug build tree. Without
    // it the manifest itself refuses to load, long before anything is
    // documented, so this job builds the framework it reads.
    process.run(
        "just",
        &["platform", "apple", "xcframework", "--profile", "debug"],
        "Apple XCFramework",
    )?;
    process.run("just", &["platform", "apple", "doc"], "Apple documentation")?;
    zip_directory(
        process,
        &ctx.root.join(&ext.release.docs_archive),
        &ctx.root.join(&ext.release.docs_asset),
    )?;
    write_checksum(&ctx.root.join(&ext.release.docs_asset))
}

pub(crate) fn wasm(process: &Process, ctx: &Ctx, ext: &KitharaExt) -> Result<()> {
    require_os("macos", "WASM release")?;
    process.run(
        "just",
        &["platform", "wasm", "build", "--profile", "release"],
        "release WASM bundle",
    )?;
    zip_directory(
        process,
        &ctx.root.join(&ext.release.wasm_dist),
        &ctx.root.join(&ext.release.wasm_asset),
    )?;
    write_checksum(&ctx.root.join(&ext.release.wasm_asset))
}

pub(crate) fn build_android(process: &Process, ctx: &Ctx, ext: &KitharaExt) -> Result<()> {
    require_os("macos", "Android release")?;
    process.run(
        "just",
        &["platform", "android", "aar"],
        "Android AAR release",
    )?;
    for name in &ext.android.aars {
        let source = ctx.root.join("android/lib/build/outputs/aar").join(name);
        let destination = ctx.root.join(name);
        copy_required(&source, &destination)?;
        write_checksum(&destination)?;
    }
    Ok(())
}

pub(crate) fn publish(ctx: &Ctx, ext: &KitharaExt) -> Result<()> {
    for variable in ["CARGO_REGISTRY_TOKEN", "GH_TOKEN", "GITLAB_TOKEN"] {
        required_env(variable)?;
    }
    let version = required_env("KITHARA_RELEASE_VERSION")?;
    let source_sha = required_env("CI_COMMIT_SHA")?;
    for name in retained_assets(&ext.release) {
        verify_checksum(&ctx.root.join(name))?;
    }

    let manifest = git_show(ctx, &source_sha, &ext.release.manifest)?;
    let manifest_version = manifest_field(&manifest, "version")?;
    if manifest_version != version {
        bail!("source manifest version is {manifest_version}, requested {version}");
    }

    release::publish_retained(ctx, &source_sha, &ctx.root)?;
    release::publish_pages_retained(ctx, &source_sha, &ctx.root)?;
    publish::publish_release(ctx)
}

fn retained_assets(config: &ReleaseConfig) -> impl Iterator<Item = &str> {
    [
        config.asset.as_str(),
        config.single_asset.as_str(),
        config.docs_asset.as_str(),
        config.wasm_asset.as_str(),
    ]
    .into_iter()
    .chain(config.platform_assets.iter().map(String::as_str))
    .filter(|name| !name.is_empty())
}

fn zip_directory(process: &Process, source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        bail!("release directory not found: {}", source.display());
    }
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("removing {}", destination.display()))?;
    }
    let parent = source
        .parent()
        .with_context(|| format!("{} has no parent", source.display()))?;
    let name = source
        .file_name()
        .with_context(|| format!("{} has no final component", source.display()))?;
    let mut command = process.command("zip");
    command
        .current_dir(parent)
        .args(["-r", "-y", "-q"])
        .arg(destination)
        .arg(name);
    process.run_command(&mut command, "zip retained release directory")
}

fn copy_required(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_file() {
        bail!("release build did not produce {}", source.display());
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "copying retained artifact {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn swift_checksum(process: &Process, path: &Path) -> Result<String> {
    let output = process
        .command("swift")
        .args(["package", "compute-checksum"])
        .arg(path)
        .output()
        .context("running swift package compute-checksum")?;
    if !output.status.success() {
        bail!(
            "swift package compute-checksum failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .context("swift checksum was not UTF-8")
        .map(|value| value.trim().to_string())
}

fn write_checksum(path: &Path) -> Result<()> {
    let checksum = sha256(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("artifact has no UTF-8 file name: {}", path.display()))?;
    fs::write(
        checksum_path(path),
        format!("{checksum}  {name}\n").as_bytes(),
    )
    .with_context(|| format!("writing checksum for {}", path.display()))
}

fn verify_checksum(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("retained release artifact is missing: {}", path.display());
    }
    let expected = fs::read_to_string(checksum_path(path))
        .with_context(|| format!("reading checksum for {}", path.display()))?
        .split_whitespace()
        .next()
        .map(str::to_string)
        .context("checksum file is empty")?;
    let actual = sha256(path)?;
    if actual != expected {
        bail!(
            "retained artifact {} has sha256 {actual}, expected {expected}",
            path.display()
        );
    }
    Ok(())
}

fn checksum_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".sha256");
    PathBuf::from(value)
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("opening retained artifact {}", path.display()))?;
    let mut digest = Sha256::new();
    io::copy(&mut file, &mut DigestWriter(&mut digest))
        .with_context(|| format!("hashing retained artifact {}", path.display()))?;
    Ok(hex::encode(digest.finalize()))
}

struct DigestWriter<'a, D>(&'a mut D);

impl<D: Digest> io::Write for DigestWriter<'_, D> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn manifest_field(manifest: &str, key: &str) -> Result<String> {
    let prefix = format!("let {key} = \"");
    manifest
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|rest| rest.strip_suffix('"'))
        .map(str::to_string)
        .with_context(|| format!("`let {key} = \"...\"` not found in release manifest"))
}

fn git_show(ctx: &Ctx, revision: &str, path: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .current_dir(&ctx.root)
        .args(["show", &format!("{revision}:{path}")])
        .output()
        .context("reading release manifest from Git")?;
    if !output.status.success() {
        bail!(
            "git show failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("release manifest from Git was not UTF-8")
}

fn required_env(name: &str) -> Result<String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("{name} is required"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_round_trip_detects_changed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("artifact.zip");
        fs::write(&artifact, b"first").unwrap();
        write_checksum(&artifact).unwrap();
        verify_checksum(&artifact).unwrap();
        fs::write(&artifact, b"second").unwrap();
        assert!(verify_checksum(&artifact).is_err());
    }

    #[test]
    fn manifest_field_requires_exact_release_declaration() {
        let manifest = "let version = \"1.2.3\"\nlet checksum = \"abc\"\n";
        assert_eq!(manifest_field(manifest, "version").unwrap(), "1.2.3");
        assert!(manifest_field("let other = \"1.2.3\"\n", "version").is_err());
    }
}
