use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};

use super::device_features;
use crate::BuildProfile;

type Features = BTreeMap<String, BTreeSet<String>>;

/// A `cargo tree` whose output is parsed, so it never inherits a forced colour:
/// a runner that sets `CARGO_TERM_COLOR=always` wraps the duplicate marker in
/// escapes, and the marker then reads as a feature name.
fn tree(root: &Path) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args(["tree", "--color", "never", "--locked"]);
    command
}

fn artifact_command(root: &Path, target: &str, profile: BuildProfile) -> Command {
    let mut command = tree(root);
    command.args([
        "-p",
        "kithara-ffi",
        "--no-default-features",
        "--features",
        &device_features(profile).replace("kithara-ffi/", ""),
        "--target",
        target,
        "--edges",
        "normal",
        "--prefix",
        "none",
        "--format",
        "{p}|{f}",
    ]);
    command
}

fn artifact(root: &Path, target: &str, profile: BuildProfile) -> Result<Features> {
    let output = artifact_command(root, target, profile)
        .output()
        .context("resolving the Android artifact composition")?;
    if !output.status.success() {
        bail!(
            "Android artifact resolution failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    parse(&String::from_utf8(output.stdout)?)
}

fn parse(tree: &str) -> Result<Features> {
    let mut packages = Features::new();
    for line in tree.lines().filter(|line| !line.is_empty()) {
        let (package, features) = line
            .split_once('|')
            .context("Cargo tree omitted the resolved feature list")?;
        let name = package
            .split_whitespace()
            .next()
            .context("Cargo tree omitted the package name")?;
        let features = features.strip_suffix(" (*)").unwrap_or(features).trim();
        packages.entry(name.to_owned()).or_default().extend(
            features
                .split(',')
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
        );
    }
    Ok(packages)
}

pub(super) fn verify(
    root: &Path,
    target: &str,
    inventory: &Command,
    evidence: &Path,
) -> Result<()> {
    let parity = evidence.join("artifact-parity.json");
    if parity.exists() {
        std::fs::remove_file(&parity)?;
    }
    if !inventory
        .get_args()
        .any(|arg| arg == "--no-default-features")
    {
        bail!("Android test inventory must disable package defaults");
    }
    let expected = artifact(root, target, BuildProfile::Release)?;
    let debug = artifact(root, target, BuildProfile::Debug)?;
    std::fs::write(
        evidence.join("product-composition.json"),
        serde_json::to_vec_pretty(&expected)?,
    )?;
    std::fs::write(
        evidence.join("artifact-composition.json"),
        serde_json::to_vec_pretty(&debug)?,
    )?;
    compare(&expected, &debug)?;
    std::fs::write(
        parity,
        serde_json::to_vec_pretty(&serde_json::json!({
            "scope": "debug-ffi-artifact",
            "release": "product-composition.json",
            "debug": "artifact-composition.json",
            "status": "passed",
        }))?,
    )?;
    let output = component_command(root, inventory)?
        .output()
        .context("resolving the Android component coverage composition")?;
    if !output.status.success() {
        bail!(
            "Android component coverage resolution failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let actual = parse(&String::from_utf8(output.stdout)?)?;
    std::fs::write(
        evidence.join("component-composition.json"),
        serde_json::to_vec_pretty(&actual)?,
    )?;
    compare_component_backends(&expected, &actual)
}

fn component_command(root: &Path, inventory: &Command) -> Result<Command> {
    let mut tree = tree(root);
    tree.args([
        "--edges",
        "normal,dev",
        "--prefix",
        "none",
        "--format",
        "{p}|{f}",
    ]);
    let mut args = inventory.get_args();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--workspace" | "--no-default-features" | "--all-features") => {
                tree.arg(arg);
            }
            Some("--features" | "--exclude" | "--target" | "-p" | "--package") => {
                tree.arg(arg)
                    .arg(args.next().context("missing Cargo selection value")?);
            }
            _ => {}
        }
    }
    Ok(tree)
}

fn compare_software_codecs(expected: &Features, actual: &Features) -> Result<()> {
    for package in actual.keys() {
        if (package.starts_with("symphonia")
            || package.starts_with("fdk-aac")
            || package.starts_with("ffmpeg")
            || package == "kithara-mpa")
            && !expected.contains_key(package)
        {
            bail!(
                "Android composition mismatch: backend dependency {package} is absent from the artifact"
            );
        }
    }
    Ok(())
}

fn compare(expected: &Features, actual: &Features) -> Result<()> {
    compare_software_codecs(expected, actual)?;
    for (package, features) in actual
        .iter()
        .filter(|(name, _)| name.starts_with("kithara"))
    {
        if package.ends_with("-tests")
            || package.starts_with("kithara-test-")
            || package == "kithara-core-test-fixtures"
            || package == "kithara-fixture-media"
        {
            continue;
        }
        let product = expected.get(package);
        // Listening artifacts use the portable encoder and recording core in
        // tests; neither package belongs to the player artifact.
        if product.is_none() && !matches!(package.as_str(), "kithara-encode" | "kithara-record") {
            bail!(
                "Android composition mismatch: {package} is in tests but absent from the artifact"
            );
        }
        let empty = BTreeSet::new();
        let product = product.unwrap_or(&empty);
        let expected_features = relevant(package, product, expected);
        let actual_features = relevant(package, features, expected);
        if expected_features != actual_features {
            bail!(
                "Android composition mismatch for {package}: artifact={expected_features:?}, tests={actual_features:?}"
            );
        }
    }
    for package in expected.keys().filter(|name| name.starts_with("kithara")) {
        if !actual.contains_key(package) {
            bail!(
                "Android composition mismatch: {package} is in the artifact but absent from tests"
            );
        }
    }
    Ok(())
}

fn compare_component_backends(expected: &Features, actual: &Features) -> Result<()> {
    compare_software_codecs(expected, actual)?;
    for package in ["kithara-decode", "kithara-net"] {
        let artifact = expected
            .get(package)
            .with_context(|| format!("Android artifact is missing {package}"))?;
        let component = actual
            .get(package)
            .with_context(|| format!("Android component coverage is missing {package}"))?;
        let artifact = relevant(package, artifact, expected);
        let component = relevant(package, component, expected);
        if artifact != component {
            bail!(
                "Android component backend mismatch for {package}: artifact={artifact:?}, components={component:?}"
            );
        }
    }
    Ok(())
}

fn relevant<'a>(
    package: &str,
    features: &'a BTreeSet<String>,
    product: &Features,
) -> BTreeSet<&'a str> {
    features
        .iter()
        .map(String::as_str)
        .filter(|feature| {
            if matches!(
                (package, *feature),
                ("kithara-ffi", "dev" | "test")
                    | ("kithara" | "kithara-host", "offline")
                    | ("kithara-platform", "signal")
                    | ("kithara-record", "assets")
            ) {
                return false;
            }
            if matches!(
                *feature,
                "default"
                    | "mock"
                    | "usdt"
                    | "flash"
                    | "hang"
                    | "tokio-net"
                    | "tokio-rt-multi-thread"
            ) {
                return false;
            }
            if package != "kithara-net"
                && (feature.starts_with("client-") || feature.starts_with("tls-"))
            {
                return false;
            }
            !(package == "kithara" && product.contains_key(&format!("kithara-{feature}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn ffi_manifest(debug: &[&str]) -> String {
        let mut features: BTreeMap<_, Vec<&str>> = device_features(BuildProfile::Debug)
            .split(',')
            .map(|feature| {
                (
                    feature.trim_start_matches("kithara-ffi/").to_owned(),
                    Vec::new(),
                )
            })
            .collect();
        features.insert("android".into(), vec!["kithara/android"]);
        features.insert("test".into(), debug.to_vec());
        let features = features
            .iter()
            .map(|(name, edges)| format!("{name:?} = {edges:?}\n"))
            .collect::<String>();
        format!(
            "[dependencies]\nkithara = {{ path = \"../facade\" }}\nkithara-decode = {{ path = \"../decode\" }}\n[features]\n{features}"
        )
    }

    fn cargo_fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir(root.join(".cargo")).unwrap();
        fs::write(root.join(".cargo/config.toml"), "[net]\noffline = true\n").unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\nmembers = [\"ffi\", \"facade\", \"decode\", \"net\", \"link\", \"component\", \"software\"]\n").unwrap();
        for (path, name, body) in [
            ("ffi", "kithara-ffi", ffi_manifest(&[])),
            ("facade", "kithara", "[dependencies]\nkithara-decode = { path = \"../decode\" }\nkithara-net = { path = \"../net\" }\nkithara-link = { path = \"../link\", optional = true }\n[features]\nandroid = [\"kithara-decode/android\", \"kithara-net/client-host\"]\nlink = [\"dep:kithara-link\"]\n".into()),
            ("decode", "kithara-decode", "[dependencies]\nsymphonia-bundle-mp3 = { path = \"../software\", optional = true }\n[features]\nandroid = []\nsymphonia = [\"dep:symphonia-bundle-mp3\"]\n".into()),
            ("net", "kithara-net", "[features]\nclient-host = []\n".into()),
            ("link", "kithara-link", String::new()),
            ("component", "kithara-host-tests", "[dependencies]\nkithara = { path = \"../facade\", features = [\"link\"] }\n".into()),
            ("software", "symphonia-bundle-mp3", String::new()),
        ] {
            let package = root.join(path);
            fs::create_dir_all(package.join("src")).unwrap();
            fs::write(package.join("src/lib.rs"), "").unwrap();
            fs::write(
                package.join("Cargo.toml"),
                format!("[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n{body}"),
            )
            .unwrap();
        }
        let output = Command::new("cargo")
            .current_dir(root)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        directory
    }

    fn component_inventory() -> Command {
        let mut inventory = Command::new("cargo");
        inventory.args([
            "nextest",
            "list",
            "--no-default-features",
            "--features",
            &device_features(BuildProfile::Debug),
            "--target",
            "aarch64-linux-android",
            "-p",
            "kithara-ffi",
            "-p",
            "kithara-host-tests",
        ]);
        inventory
    }

    #[test]
    fn resolved_component_link_does_not_contaminate_the_debug_artifact() {
        let directory = cargo_fixture();
        let root = directory.path();
        let evidence = root.join("evidence");
        fs::create_dir(&evidence).unwrap();
        let inventory = component_inventory();

        let release = artifact(root, "aarch64-linux-android", BuildProfile::Release).unwrap();
        let output = component_command(root, &inventory)
            .unwrap()
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let components = parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
        assert!(components.contains_key("kithara-link"));
        assert!(components["kithara"].contains("link"));
        assert!(compare(&release, &components).is_err());

        verify(root, "aarch64-linux-android", &inventory, &evidence).unwrap();
        let debug: Features =
            serde_json::from_slice(&fs::read(evidence.join("artifact-composition.json")).unwrap())
                .unwrap();
        assert!(!debug.contains_key("kithara-link"));
        assert!(!debug["kithara"].contains("link"));
        compare(&release, &debug).unwrap();
        let recorded: Features =
            serde_json::from_slice(&fs::read(evidence.join("component-composition.json")).unwrap())
                .unwrap();
        assert_eq!(recorded, components);
        let parity: serde_json::Value =
            serde_json::from_slice(&fs::read(evidence.join("artifact-parity.json")).unwrap())
                .unwrap();
        assert_eq!(parity["scope"], "debug-ffi-artifact");
        assert_eq!(parity["status"], "passed");

        fs::write(
            root.join("ffi/Cargo.toml"),
            format!(
                "[package]\nname = \"kithara-ffi\"\nversion = \"0.0.0\"\nedition = \"2024\"\n{}",
                ffi_manifest(&["kithara/link"])
            ),
        )
        .unwrap();
        let error = verify(root, "aarch64-linux-android", &inventory, &evidence)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Android composition mismatch"), "{error}");
        let leaked: Features =
            serde_json::from_slice(&fs::read(evidence.join("artifact-composition.json")).unwrap())
                .unwrap();
        assert!(leaked.contains_key("kithara-link"));
        assert!(leaked["kithara"].contains("link"));
        assert!(!evidence.join("artifact-parity.json").exists());
    }

    #[test]
    fn a_resolved_debug_artifact_cannot_add_a_software_decoder() {
        let directory = cargo_fixture();
        let root = directory.path();
        let evidence = root.join("evidence");
        fs::create_dir(&evidence).unwrap();
        fs::write(
            root.join("ffi/Cargo.toml"),
            format!(
                "[package]\nname = \"kithara-ffi\"\nversion = \"0.0.0\"\nedition = \"2024\"\n{}",
                ffi_manifest(&["kithara-decode/symphonia"])
            ),
        )
        .unwrap();

        let error = verify(
            root,
            "aarch64-linux-android",
            &component_inventory(),
            &evidence,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("backend dependency symphonia-bundle-mp3"),
            "{error}"
        );
        assert!(!evidence.join("artifact-parity.json").exists());
    }

    #[test]
    fn component_contracts_do_not_redefine_artifact_parity() {
        let artifact = parse("kithara v1|android\nkithara-ffi v1|android,uniffi\nkithara-decode v1|android\nkithara-net v1|client-host\nkithara-beat v1|\nkithara-dsp v1|\nkithara-platform v1|\nkithara-stretch v1|stretch-signalsmith\nkithara-warp v1|render,stretch-signalsmith\n").unwrap();
        let components = parse("kithara v1|android,link\nkithara-ffi v1|android,dev,test,uniffi\nkithara-decode v1|android,mock,usdt\nkithara-net v1|client-host,mock\nkithara-link v1|stretch-signalsmith\nkithara-beat v1|dsp\nkithara-dsp v1|spectrum\nkithara-platform v1|tokio-process\nkithara-stretch v1|stretch-identity,stretch-signalsmith\nkithara-warp v1|render,stretch-identity,stretch-signalsmith\n").unwrap();

        assert!(compare(&artifact, &components).is_err());
        compare_component_backends(&artifact, &components).unwrap();
        compare(&artifact, &artifact).unwrap();
    }

    #[test]
    fn component_coverage_keeps_the_artifact_decoder_and_transport() {
        let artifact = parse("kithara-decode v1|android\nkithara-net v1|client-host\n").unwrap();
        for components in [
            "kithara-decode v1|\nkithara-net v1|client-host\n",
            "kithara-decode v1|android,symphonia\nkithara-net v1|client-host\n",
            "kithara-decode v1|android\nkithara-net v1|client-host,client-reqwest\n",
            "kithara-decode v1|android\nkithara-net v1|client-reqwest\n",
            "kithara-decode v1|android\n",
            "kithara-net v1|client-host\n",
            "kithara-decode v1|android\nkithara-net v1|client-host\nsymphonia-bundle-mp3 v1|\n",
        ] {
            assert!(compare_component_backends(&artifact, &parse(components).unwrap()).is_err());
        }
    }

    #[test]
    fn debug_entry_points_are_checked_against_release_backends() {
        let product =
            parse("kithara-ffi v1|android,uniffi\nkithara-decode v1|android\nkithara-host v1|\n")
                .unwrap();
        let debug = parse("kithara-ffi v1|android,dev,test,uniffi\nkithara-decode v1|android,usdt\nkithara-host v1|offline\n").unwrap();
        compare(&product, &debug).unwrap();
        let mut leaked = debug;
        leaked
            .get_mut("kithara-decode")
            .unwrap()
            .insert("symphonia".into());
        assert!(compare(&product, &leaked).is_err());
    }

    #[test]
    fn test_support_cannot_enable_a_software_decoder() {
        let product = parse("kithara-decode v1|android\nkithara-net v1|client-wreq\n").unwrap();
        let leaked =
            parse("kithara-decode v1|android,symphonia\nkithara-net v1|client-wreq\n").unwrap();
        assert!(compare(&product, &leaked).is_err());
        let direct =
            parse("kithara-decode v1|android\nkithara-net v1|client-wreq\nsymphonia v1|\n")
                .unwrap();
        assert!(compare(&product, &direct).is_err());
        let instrumented = parse("kithara-decode v1|android,mock,usdt\nkithara-net v1|client-wreq,mock\nkithara-test-dylib v1|\n").unwrap();
        compare(&product, &instrumented).unwrap();
    }

    #[test]
    fn product_mpeg_parser_does_not_authorize_a_software_codec() {
        let product =
            parse("kithara-decode v1|android\nkithara-mpa v1|\nsymphonia-core v1|\n").unwrap();
        compare(&product, &product).unwrap();
        let mut leaked = product.clone();
        leaked.insert("symphonia-bundle-mp3".into(), BTreeSet::new());
        assert!(compare(&product, &leaked).is_err());
    }

    #[test]
    fn test_helpers_cannot_add_a_second_network_backend_or_app() {
        let product = parse("kithara-net v1|client-wreq\n").unwrap();
        for tree in [
            "kithara-net v1|client-wreq,client-reqwest\n",
            "kithara-net v1|client-wreq\nkithara-app v1|\n",
        ] {
            assert!(compare(&product, &parse(tree).unwrap()).is_err());
        }
    }

    #[test]
    fn parsed_trees_ask_cargo_for_uncoloured_output() {
        let args: Vec<_> = tree(Path::new("."))
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2).any(|pair| pair == ["--color", "never"]),
            "cargo tree must not inherit a forced colour: {args:?}"
        );
        let coloured =
            parse("kithara v1|warp \u{1b}[33m\u{1b}[2m(*)\u{1b}[39m\u{1b}[22m\n").unwrap();
        assert_ne!(
            coloured,
            parse("kithara v1|warp (*)\n").unwrap(),
            "a coloured duplicate marker is not stripped, so colour must be off"
        );
    }

    #[test]
    fn fixture_types_do_not_allow_an_encoder_backend() {
        let product = Features::new();
        compare(&product, &parse("kithara-encode v1|\n").unwrap()).unwrap();
        assert!(compare(&product, &parse("kithara-encode v1|ffmpeg\n").unwrap()).is_err());
    }

    #[test]
    fn listening_artifact_recording_does_not_change_product_composition() {
        let product = parse("kithara-assets v1|\n").unwrap();
        let tests =
            parse("kithara-assets v1|\nkithara-encode v1|\nkithara-record v1|assets\n").unwrap();
        compare(&product, &tests).unwrap();
        assert!(
            compare(
                &product,
                &parse("kithara-assets v1|extra\nkithara-record v1|assets\n").unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn fixture_media_is_test_only() {
        compare(
            &Features::new(),
            &parse("kithara-fixture-media v1|native\n").unwrap(),
        )
        .unwrap();
    }
}
