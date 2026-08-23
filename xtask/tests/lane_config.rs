use std::{fs, path::Path};

// A browser lane's name is a promise about what ran. The harness reads
// `KITHARA_SELENIUM_BROWSER` and falls back to chrome when nothing sets it, so
// the lane that names a browser has to name it in its own configuration -
// otherwise `--lane=selenium-firefox` reports Firefox and runs Chrome.
#[test]
fn a_selenium_lane_names_its_browser_in_its_own_environment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let config: toml::Value = toml::from_str(
        &fs::read_to_string(root.join(".config/xtask.toml")).expect("xtask config is readable"),
    )
    .expect("xtask config is valid TOML");
    let lanes = config["test"]["lanes"]
        .as_table()
        .expect("test lanes are a table");

    let mut checked = 0;
    for (name, lane) in lanes {
        let Some(browser) = name.strip_prefix("selenium-") else {
            continue;
        };
        let named = lane
            .get("env")
            .and_then(|env| env.get("KITHARA_SELENIUM_BROWSER"))
            .and_then(toml::Value::as_str);
        assert_eq!(
            named,
            Some(browser),
            "lane `{name}` must name its browser: KITHARA_SELENIUM_BROWSER"
        );
        checked += 1;
    }

    assert!(checked > 0, "no selenium lane is configured to check");
}

// The coverage floor gates a report that nothing else gates, so it has to be
// enforced where it is declared, and declared once. Two copies is what let the
// lane pass 80 while the recipe defaulted to 80: agreeing by accident, and one
// edit away from gating a local run and CI at different bars.
#[test]
fn the_coverage_floor_is_declared_once_and_enforced_where_it_is_declared() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let recipe = fs::read_to_string(root.join(".config/just/test.just"))
        .expect("the test recipes are readable");

    assert!(
        recipe.contains(r#"COVERAGE_MIN="${COVERAGE_MIN:-76}""#),
        "the coverage floor is declared in .config/just/test.just"
    );
    assert!(
        recipe.contains(r#"--fail-under-lines "$COVERAGE_MIN""#),
        "the declared floor is the one the report is failed under"
    );

    let lane = fs::read_to_string(root.join("xtask/src/ci/lane/linux.rs"))
        .expect("the Linux lanes are readable");
    assert!(
        !lane.contains("COVERAGE_MIN"),
        "the coverage lane inherits the floor instead of keeping a second copy"
    );
}
