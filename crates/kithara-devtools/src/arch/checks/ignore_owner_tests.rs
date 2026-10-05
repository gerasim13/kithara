use std::fs;

use cargo_metadata::MetadataCommand;

use super::{Context, registry};
use crate::{
    arch::config::ArchConfig,
    common::{scope::Scope, violation::Violation},
};

fn ownership_findings(source: &str) -> Vec<Violation> {
    let check = registry()
        .into_iter()
        .find(|check| check.id() == "arch.ignored-test-owner")
        .expect("the architecture gate must enforce ownership of ignored tests");
    let temp = tempfile::tempdir().expect("temporary workspace");
    let member = temp.path().join("crates/fixture");
    fs::create_dir_all(member.join("src")).expect("fixture source directory");
    fs::create_dir_all(temp.path().join(".config/just")).expect("fixture config directory");
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/fixture\"]\nresolver = \"3\"\n",
    )
    .expect("workspace manifest");
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("member manifest");
    fs::write(member.join("src/lib.rs"), source).expect("ignored source fixture");
    fs::write(
        temp.path().join(".config/xtask.toml"),
        r#"[test]
default_lane = "tooling"
default_backend = "none"
[test.net_backends.none]
features = []
[test.lanes.tooling]
cargo.packages = ["fixture"]
undeclared_toggles = ["flash", "no-block"]
"#,
    )
    .expect("configured fixture lane");
    fs::write(
        temp.path().join("justfile"),
        "mod test '.config/just/test.just'\n",
    )
    .expect("fixture command surface");
    fs::write(
        temp.path().join(".config/just/test.just"),
        "run *ARGS:\n    true\n",
    )
    .expect("fixture test recipe");
    let metadata = MetadataCommand::new()
        .manifest_path(temp.path().join("Cargo.toml"))
        .no_deps()
        .exec()
        .expect("metadata for the source fixture");
    let config = ArchConfig::default();
    let scope = Scope::default();
    let context = Context::new(&config, &metadata, temp.path(), &scope);
    check.run(&context).expect("source ownership check")
}

#[test]
fn ignored_functions_without_an_execution_or_defect_owner_are_rejected() {
    for attribute in ["#[ignore]", "#[ignore = \"waiting for a fix\"]"] {
        let source = format!("#[test]\n{attribute}\nfn unowned() {{}}\n");
        let findings = ownership_findings(&source);
        assert_eq!(findings.len(), 1, "unowned declaration: {attribute}");
        assert!(findings[0].key.contains("unowned"), "{findings:?}");
    }
}

#[test]
fn ignored_functions_name_a_real_lane_recipe_or_issue() {
    let source = r#"
#[test]
#[ignore = "lane: tooling; subprocess entrypoint"]
fn child() {}
#[test]
#[ignore = "run: just test run --lane=tooling --run-ignored=only"]
fn manual() {}
#[test]
#[ignore = "issue: https://github.com/zvuk/kithara/issues/548; nightly: red"]
fn pinned() {}
#[test]
#[ignore = "issue: https://github.com/zvuk/kithara/issues/548; nightly: flake"]
fn measured_flake() {}
"#;
    assert!(ownership_findings(source).is_empty());
}

#[test]
fn an_unknown_lane_recipe_or_nightly_kind_is_not_an_owner() {
    for reason in [
        "lane: missing",
        "run: just test missing",
        "issue: https://github.com/zvuk/kithara/pull/548",
        "issue: https://github.com/zvuk/kithara/issues/548; nightly: maybe",
    ] {
        let source = format!("#[test]\n#[ignore = {reason:?}]\nfn invalid() {{}}\n");
        assert_eq!(ownership_findings(&source).len(), 1, "{reason}");
    }
}

#[test]
fn conditional_ignore_attributes_are_owned_but_comment_and_string_text_is_not_a_test() {
    let source = r##"
// #[ignore] is source text in a comment.
const EXAMPLE: &str = "#[ignore] fn documentation_example() {}";
#[test]
#[cfg_attr(unix, ignore = "waiting for a fix")]
fn conditional() {}
"##;
    let findings = ownership_findings(source);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].key.contains("conditional"), "{findings:?}");
}
