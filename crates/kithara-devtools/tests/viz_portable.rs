use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{FromArgMatches, Subcommand};
use kithara_devtools::{CoreCommand, Ctx};
use tempfile::tempdir;

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("create fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("fixture entry");
        let destination = target.join(entry.file_name());
        if entry.file_type().expect("fixture type").is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).expect("copy fixture file");
        }
    }
}

fn viz_command(runtime: &str) -> CoreCommand {
    let command = CoreCommand::augment_subcommands(clap::Command::new("xtask"));
    let matches = command
        .try_get_matches_from([
            "xtask",
            "viz",
            "--crate",
            "flow",
            "--lod",
            "3",
            "--semantic",
            "off",
            "--runtime",
            runtime,
        ])
        .expect("parse viz command");
    CoreCommand::from_arg_matches(&matches).expect("build viz command")
}

fn workspace_lod_command(lod: &str) -> CoreCommand {
    workspace_lod_command_with_runtime(lod, "off")
}

fn workspace_lod_command_with_runtime(lod: &str, runtime: &str) -> CoreCommand {
    let command = CoreCommand::augment_subcommands(clap::Command::new("xtask"));
    let matches = command
        .try_get_matches_from([
            "xtask",
            "viz",
            "--lod",
            lod,
            "--semantic",
            "off",
            "--runtime",
            runtime,
        ])
        .expect("parse workspace LOD command");
    CoreCommand::from_arg_matches(&matches).expect("build workspace LOD command")
}

fn filtered_workspace_command() -> CoreCommand {
    let command = CoreCommand::augment_subcommands(clap::Command::new("xtask"));
    let matches = command
        .try_get_matches_from([
            "xtask",
            "viz",
            "--lod",
            "1",
            "--semantic",
            "off",
            "--runtime",
            "off",
            "--exclude-crate",
            "unrelated",
            "--exclude-module",
            "flow::runtime",
        ])
        .expect("parse filtered workspace command");
    CoreCommand::from_arg_matches(&matches).expect("build filtered workspace command")
}

fn crate_lod_command(package: &str, lod: &str) -> CoreCommand {
    let command = CoreCommand::augment_subcommands(clap::Command::new("xtask"));
    let matches = command
        .try_get_matches_from([
            "xtask",
            "viz",
            "--crate",
            package,
            "--lod",
            lod,
            "--semantic",
            "off",
            "--runtime",
            "off",
        ])
        .expect("parse crate LOD command");
    CoreCommand::from_arg_matches(&matches).expect("build crate LOD command")
}

fn auto_lod_command(scope: &[&str]) -> CoreCommand {
    let command = CoreCommand::augment_subcommands(clap::Command::new("xtask"));
    let mut arguments = vec!["xtask", "viz", "--semantic", "off", "--runtime", "off"];
    arguments.extend_from_slice(scope);
    let matches = command
        .try_get_matches_from(arguments)
        .expect("parse automatic LOD command");
    CoreCommand::from_arg_matches(&matches).expect("build automatic LOD command")
}

#[test]
fn workspace_lod_zero_shows_every_crate_without_internal_nodes() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest).expect("load fixture context");

    kithara_devtools::run(&workspace_lod_command("0"), &ctx).expect("workspace LOD 0 run");

    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let document = fs::read_to_string(&output).expect("architecture document");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let projection: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("projection.json")).expect("projection"),
    )
    .expect("projection JSON");
    assert!(document.contains("consumer"));
    assert!(document.contains("conditional"));
    assert!(document.contains("flow"));
    assert!(document.contains("support"));
    assert!(document.contains("unrelated"));
    assert!(document.contains("depends on"));
    assert!(!document.contains("[\"start\"]"));
    assert!(!document.contains("[\"worker\"]"));
    assert!(!document.contains("Arc&lt;"));
    assert_eq!(manifest["lod"], 0);
    assert_eq!(manifest["hidden_nodes"], 0);
    assert!(
        projection["edges"]
            .as_array()
            .expect("projection edges")
            .iter()
            .any(|edge| {
                edge["source"]["package"] == "flow"
                    && edge["target"]["package"] == "conditional"
                    && edge["style"] == "conditional"
            })
    );
}

#[test]
fn workspace_filters_remove_crates_modules_edges_and_report_findings() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest_path = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest_path).expect("load fixture context");

    kithara_devtools::run(&filtered_workspace_command(), &ctx).expect("filtered workspace run");

    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let document = fs::read_to_string(&output).expect("architecture document");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let projection: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("projection.json")).expect("projection"),
    )
    .expect("projection JSON");

    assert!(!document.contains("unrelated"));
    assert!(!document.contains("[\"runtime\"]"));
    assert!(
        projection["nodes"]
            .as_array()
            .expect("projection nodes")
            .iter()
            .all(|node| {
                node["id"]["package"] != "unrelated" && node["id"]["module"] != "runtime"
            })
    );
    assert!(
        projection["edges"]
            .as_array()
            .expect("projection edges")
            .iter()
            .all(|edge| {
                edge["source"]["package"] != "unrelated"
                    && edge["target"]["package"] != "unrelated"
                    && edge["source"]["module"] != "runtime"
                    && edge["target"]["module"] != "runtime"
            })
    );
    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(
        manifest["filters"]["exclude_crates"],
        serde_json::json!(["unrelated"])
    );
    assert_eq!(
        manifest["filters"]["exclude_modules"],
        serde_json::json!(["flow::runtime"])
    );
    assert!(
        manifest["filters"]["excluded_nodes"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(
        manifest["filters"]["excluded_edges"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
}

#[test]
fn exclusion_filters_reject_invalid_or_empty_scope() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest_path = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest_path).expect("load fixture context");

    let selected = kithara_devtools::run(
        &auto_lod_command(&["--crate", "flow", "--exclude-crate", "flow"]),
        &ctx,
    )
    .expect_err("selected excluded crate");
    assert!(selected.to_string().contains("selected workspace package"));

    let selected_module = kithara_devtools::run(
        &auto_lod_command(&[
            "--crate",
            "flow",
            "--module",
            "runtime",
            "--exclude-module",
            "flow::runtime",
        ]),
        &ctx,
    )
    .expect_err("selected excluded module");
    assert!(
        selected_module
            .to_string()
            .contains("selected module scope")
    );

    let unknown = kithara_devtools::run(&auto_lod_command(&["--exclude-crate", "missing"]), &ctx)
        .expect_err("unknown exact excluded crate");
    assert!(
        unknown
            .to_string()
            .contains("workspace package not found for --exclude-crate")
    );

    let empty = kithara_devtools::run(&auto_lod_command(&["--exclude-crate", "*"]), &ctx)
        .expect_err("empty projection");
    assert!(empty.to_string().contains("removed every node"));
}

#[test]
fn automatic_lod_follows_workspace_crate_and_module_scope() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest_path = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest_path).expect("load fixture context");
    let output = temp
        .path()
        .join("target/architecture/working-tree/manifest.json");

    for (scope, expected) in [
        (&[][..], 0),
        (&["--crate", "flow"][..], 2),
        (&["--crate", "flow", "--module", "runtime"][..], 3),
    ] {
        kithara_devtools::run(&auto_lod_command(scope), &ctx).expect("automatic LOD run");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).expect("manifest")).expect("manifest JSON");
        assert_eq!(manifest["lod"], expected);
    }
}

#[test]
fn crate_lod_two_groups_methods_into_architectural_abstractions() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest).expect("load fixture context");

    kithara_devtools::run(&crate_lod_command("flow", "2"), &ctx).expect("crate LOD 2 run");

    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let document = fs::read_to_string(&output).expect("architecture document");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let projection: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("projection.json")).expect("projection"),
    )
    .expect("projection JSON");

    assert!(document.contains("[\"Primary\"]"));
    assert!(document.contains("[\"Work\"]"));
    assert!(document.contains("[\"runtime functions\"]"));
    assert!(document.contains("implements"));
    assert!(document.contains("|calls|"));
    assert!(!document.contains("[\"FixtureOnly\"]"));
    assert!(!document.contains("[\"InlineNoise\"]"));
    assert!(!document.contains("[\"testing\"]"));
    assert!(!document.contains("[\"fn start()\"]"));
    assert!(!document.contains("[\"fn worker()\"]"));
    assert!(document.contains("Architecture contours:"));
    assert!(document.contains("abstractions"));
    assert_eq!(manifest["schema_version"], 3);
    assert_eq!(manifest["lod"], 2);
    let lifted_call = projection["edges"]
        .as_array()
        .expect("projection edges")
        .iter()
        .find(|edge| {
            edge["kind"] == "calls"
                && edge["source"]["symbol"]
                    .as_str()
                    .is_some_and(|symbol| symbol.contains("Primary"))
        })
        .expect("lifted call from Primary");
    assert!(
        lifted_call["details"]
            .as_array()
            .expect("call details")
            .iter()
            .any(|detail| {
                detail.as_str().is_some_and(|detail| {
                    detail.contains("Primary::run") && detail.contains("recurse")
                })
            })
    );
    assert!(lifted_call["count"].as_u64().is_some_and(|count| count > 0));
    assert!(
        lifted_call["origins"]
            .as_array()
            .is_some_and(|origins| !origins.is_empty())
    );
}

#[test]
fn crate_lod_three_reveals_constructor_and_boundary_methods() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest).expect("load fixture context");

    kithara_devtools::run(&crate_lod_command("flow", "3"), &ctx).expect("crate LOD 3 run");

    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let document = fs::read_to_string(&output).expect("architecture document");

    assert!(document.contains("subgraph "));
    assert!(document.contains("[\"fn Primary::new()\"]"));
    assert!(document.contains("[\"fn Primary::run()\"]"));
    assert!(document.contains("[\"fn Work::run()\"]"));
    assert!(document.contains("[\"fn recurse()\"]"));
    assert!(document.contains("[\"fn invoke()\"]"));
    assert!(document.contains("[\"fn worker()\"]"));
    assert!(!document.contains("[\"fn local()\"]"));
}

#[test]
fn crate_lod_one_and_four_bound_the_detail_range() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest).expect("load fixture context");
    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");

    kithara_devtools::run(&crate_lod_command("flow", "1"), &ctx).expect("crate LOD 1 run");
    let modules = fs::read_to_string(&output).expect("module architecture document");
    assert!(modules.contains("[\"runtime\"]"));
    assert!(!modules.contains("[\"Primary\"]"));
    assert!(!modules.contains("[\"fn start()\"]"));

    kithara_devtools::run(&crate_lod_command("flow", "4"), &ctx).expect("crate LOD 4 run");
    let full = fs::read_to_string(&output).expect("full architecture document");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let projection: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("projection.json")).expect("projection"),
    )
    .expect("projection JSON");
    assert_eq!(manifest["partition"]["state"], "partitioned");
    assert_eq!(
        manifest["partition"]["covered_nodes"],
        projection["nodes"]
            .as_array()
            .expect("projection nodes")
            .len()
    );
    let mut rendered = full;
    for page in manifest["partition"]["pages"]
        .as_array()
        .expect("partition pages")
    {
        let page = output
            .parent()
            .expect("architecture output directory")
            .join(page["file"].as_str().expect("page file"));
        assert!(page.is_file());
        rendered.push_str(&fs::read_to_string(page).expect("contour document"));
    }
    assert!(rendered.contains("[\"Primary\"]"));
    assert!(rendered.contains("[\"fn start()\"]"));
    assert!(rendered.contains("[\"fn local()\"]"));
    assert!(rendered.contains("Arc&lt;"));
}

#[test]
fn portable_workspace_produces_stable_mermaid_artifacts() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest).expect("load fixture context");

    kithara_devtools::run(&viz_command("off"), &ctx).expect("first viz run");
    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let first = fs::read_to_string(&output).expect("architecture document");
    assert!(first.contains("flowchart LR"));
    assert!(first.contains("consumer"));
    assert!(first.contains("flow"));
    assert!(first.contains("support"));
    assert!(first.contains("depends on"));
    assert!(!first.contains("unrelated"));
    assert!(first.contains("start"));
    assert!(first.contains("worker"));
    assert!(first.contains("Arc&lt;"));
    assert!(first.contains("## Limitations"));
    assert!(output.with_file_name("graph.json").is_file());
    assert!(output.with_file_name("manifest.json").is_file());

    kithara_devtools::run(&viz_command("off"), &ctx).expect("second viz run");
    let second = fs::read_to_string(output).expect("architecture document");
    assert_eq!(first, second);
}

#[test]
fn configured_runtime_scenario_enriches_the_same_graph() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    let manifest = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest).expect("load fixture context");

    kithara_devtools::run(&viz_command("auto"), &ctx).expect("runtime viz run");

    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let document = fs::read_to_string(&output).expect("architecture document");
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    assert!(document.contains("classDef observed"));
    assert_eq!(manifest["status"], "runtime-enriched");
    assert_eq!(manifest["runtime"]["scenarios"][0]["state"], "complete");
    assert!(
        output
            .with_file_name("traces")
            .join("flow-runtime.jsonl")
            .is_file()
    );
}

#[test]
fn excluded_test_harness_can_still_produce_runtime_evidence() {
    let temp = tempdir().expect("tempdir");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/architecture-workspace");
    copy_tree(&fixture, temp.path());
    fs::write(
        temp.path().join(".config/xtask.toml"),
        r#"
[project]
name = "portable-flow"

[architecture.filters]
exclude_crates = ["harness"]

[[architecture.runtime.scenarios]]
name = "harness-runtime"
command = "test"
package = "harness"
test = "architecture"
filter = "writes_architecture_trace"
timeout_secs = 30
"#,
    )
    .expect("filtered runtime config");
    let manifest_path = temp.path().join("Cargo.toml");
    let ctx = Ctx::load_from_manifest(&manifest_path).expect("load fixture context");

    kithara_devtools::run(&workspace_lod_command_with_runtime("3", "auto"), &ctx)
        .expect("runtime evidence from excluded harness");

    let output = temp
        .path()
        .join("target/architecture/working-tree/architecture.md");
    let document = fs::read_to_string(&output).expect("architecture document");
    let graph: serde_json::Value =
        serde_json::from_slice(&fs::read(output.with_file_name("graph.json")).expect("graph"))
            .expect("graph JSON");
    let projection: serde_json::Value = serde_json::from_slice(
        &fs::read(output.with_file_name("projection.json")).expect("projection"),
    )
    .expect("projection JSON");

    assert!(document.contains("classDef observed"));
    assert!(!document.contains("[\"harness\"]"));
    assert!(
        graph["nodes"]
            .as_array()
            .expect("graph nodes")
            .iter()
            .any(|node| node["id"]["package"] == "harness")
    );
    assert!(
        projection["nodes"]
            .as_array()
            .expect("projection nodes")
            .iter()
            .all(|node| node["id"]["package"] != "harness")
    );
}
