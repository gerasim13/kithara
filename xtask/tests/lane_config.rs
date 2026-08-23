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

// The coverage bar gates a report that nothing else gates, so it has to be
// enforced where it is declared, and declared once. Two copies is what let the
// lane pass 80 while the recipe defaulted to 80: agreeing by accident, and one
// edit away from gating a local run and CI at different bars. The bar is above
// what the workspace holds today on purpose - the UI surface gets its own tests
// - so this also fixes the number a green lane may not be bought with.
#[test]
fn the_coverage_bar_is_declared_once_and_enforced_where_it_is_declared() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a workspace root");
    let recipe = fs::read_to_string(root.join(".config/just/test.just"))
        .expect("the test recipes are readable");

    assert!(
        recipe.contains(r#"COVERAGE_MIN="${COVERAGE_MIN:-80}""#),
        "the coverage bar is declared in .config/just/test.just"
    );
    assert!(
        recipe.contains(r#"--fail-under-lines "$COVERAGE_MIN""#),
        "the declared bar is the one the report is failed under"
    );

    let config: toml::Value = toml::from_str(
        &fs::read_to_string(root.join(".config/xtask.toml")).expect("xtask config is readable"),
    )
    .expect("xtask config is valid TOML");
    let lanes = config["ext"]["ci"]["lanes"]
        .as_table()
        .expect("CI lanes are a table");

    let mut runs_the_recipe = 0;
    for (name, lane) in lanes {
        let steps = lane
            .get("steps")
            .and_then(toml::Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for step in steps {
            assert!(
                step.get("env")
                    .and_then(|env| env.get("COVERAGE_MIN"))
                    .is_none(),
                "lane `{name}` keeps a second copy of the bar instead of inheriting it"
            );
            let args: Vec<&str> = step
                .get("args")
                .and_then(toml::Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .filter_map(toml::Value::as_str)
                .collect();
            if args == ["test", "coverage"] {
                runs_the_recipe += 1;
            }
        }
    }

    assert_eq!(
        runs_the_recipe, 1,
        "exactly one lane runs the coverage recipe that carries the bar"
    );
}
