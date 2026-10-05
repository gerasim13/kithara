use anyhow::{Context, Result, bail};
use kithara_devtools::common::project::{ProjectConfig, TestRunner};

use crate::config::CiLaneConfig;

pub(crate) fn validate(
    lane: &CiLaneConfig,
    kind: &str,
    expression: &str,
    project: &ProjectConfig,
) -> Result<bool> {
    if expression.trim().is_empty() {
        bail!("a test filter must contain a nextest expression");
    }
    let mut selected = false;
    for step in &lane.steps {
        let role = step.program.as_deref().unwrap_or(&lane.program);
        let mut args = step.args_by_kind.get(kind).unwrap_or(&step.args).clone();
        selected |= apply(role, &mut args, expression, project)?;
    }
    Ok(selected)
}

pub(super) fn apply(
    role: &str,
    args: &mut Vec<String>,
    expression: &str,
    project: &ProjectConfig,
) -> Result<bool> {
    if role != "just" || !args.get(..2).is_some_and(|prefix| prefix == ["test", "run"]) {
        return Ok(false);
    }
    let lane_name = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--lane="))
        .or_else(|| {
            args.windows(2)
                .find(|pair| pair[0] == "--lane")
                .map(|pair| pair[1].as_str())
        })
        .unwrap_or(&project.test.default_lane);
    let lane = project
        .test
        .lanes
        .get(lane_name)
        .with_context(|| format!("test filter names an unknown test lane `{lane_name}`"))?;
    if !matches!(&lane.runner, TestRunner::Nextest(_)) {
        return Ok(false);
    }
    intersect(args, expression)?;
    Ok(true)
}

/// Every caller filter is a union member in the test harness. Narrow each
/// member so adding the CI expression cannot broaden a declared selection.
fn intersect(args: &mut Vec<String>, expression: &str) -> Result<()> {
    const VALUED: [&str; 3] = ["-E", "--filterset", "--filter-expr"];
    const ATTACHED: [&str; 4] = ["--filterset=", "--filter-expr=", "-E=", "-E"];
    let mut narrowed = false;
    let mut iter = args.iter_mut().skip(2);
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        if VALUED.contains(&arg.as_str()) {
            let previous = iter
                .next()
                .with_context(|| format!("`{arg}` needs a filterset after it"))?;
            *previous = format!("({previous}) & ({expression})");
            narrowed = true;
        } else if let Some((prefix, previous)) = ATTACHED
            .iter()
            .find_map(|prefix| arg.strip_prefix(prefix).map(|value| (*prefix, value)))
        {
            *arg = format!("{prefix}({previous}) & ({expression})");
            narrowed = true;
        }
    }
    if !narrowed {
        let separator = args.iter().position(|arg| arg == "--").unwrap_or(args.len());
        args.splice(separator..separator, ["-E".to_owned(), expression.to_owned()]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectConfig {
        toml::from_str(
            r#"
[test]
default_lane = "workspace"
[test.lanes.workspace]
[test.lanes.tooling]
[test.lanes.doc.runner.cargo]
doc = true
"#,
        )
        .expect("test runner configuration")
    }

    #[test]
    fn a_filter_is_one_argument_and_preserves_the_declared_test_arguments() {
        let expression = "test(a) or test(b); $(touch unexpected)";
        let mut args = ["test", "run", "--flash=off"]
            .map(str::to_owned)
            .to_vec();
        assert!(apply("just", &mut args, expression, &project()).unwrap());
        assert_eq!(args[3..], ["-E", expression]);
        assert_eq!(args[2], "--flash=off");
    }

    #[test]
    fn a_filter_uses_the_declared_nextest_test_lane() {
        for selector in [vec!["--lane=tooling"], vec!["--lane", "tooling"]] {
            let mut args: Vec<String> = [vec!["test", "run"], selector]
                .concat()
                .into_iter()
                .map(str::to_owned)
                .collect();
            assert!(apply("just", &mut args, "test(contract)", &project()).unwrap());
            assert_eq!(args[args.len() - 2..], ["-E", "test(contract)"]);
        }
    }

    #[test]
    fn a_global_filter_leaves_cargo_tests_unchanged_and_rejects_unknown_lanes() {
        let mut args = ["test", "run", "--lane=doc"].map(str::to_owned).to_vec();
        let before = args.clone();
        assert!(!apply("just", &mut args, "test(contract)", &project()).unwrap());
        assert_eq!(args, before);
        args[2] = "--lane=missing".to_owned();
        assert!(apply("just", &mut args, "test(contract)", &project()).is_err());
    }

    #[test]
    fn declared_caller_filter_union_is_narrowed_without_changing_other_arguments() {
        for (flag, attached) in [
            ("-E", "-Etest(second)"),
            ("--filterset", "--filterset=test(second)"),
            ("--filter-expr", "--filter-expr=test(second)"),
        ] {
            let mut args = ["test", "run", flag, "test(first)", attached, "--flash=off"]
                .map(str::to_owned)
                .to_vec();
            assert!(apply("just", &mut args, "binary(contract)", &project()).unwrap());
            assert_eq!(args[3], "(test(first)) & (binary(contract))");
            let prefix = attached.strip_suffix("test(second)").unwrap();
            assert_eq!(args[4], format!("{prefix}(test(second)) & (binary(contract))"));
            assert_eq!(args[5], "--flash=off");
        }
    }

    #[test]
    fn a_new_filter_precedes_the_binary_argument_separator() {
        let mut args = ["test", "run", "--", "-E", "binary_argument"]
            .map(str::to_owned)
            .to_vec();
        assert!(apply("just", &mut args, "test(contract)", &project()).unwrap());
        assert_eq!(
            args,
            ["test", "run", "-E", "test(contract)", "--", "-E", "binary_argument"]
        );
    }

    #[test]
    fn an_existing_filter_is_narrowed_without_touching_binary_arguments() {
        let mut args = ["test", "run", "-E", "test(owned)", "--", "-E", "binary_argument"]
            .map(str::to_owned)
            .to_vec();
        assert!(apply("just", &mut args, "test(contract)", &project()).unwrap());
        assert_eq!(args[3], "(test(owned)) & (test(contract))");
        assert_eq!(args[4..], ["--", "-E", "binary_argument"]);
    }

    #[test]
    fn a_missing_declared_filter_value_is_refused() {
        let mut args = ["test", "run", "-E"].map(str::to_owned).to_vec();
        assert!(apply("just", &mut args, "test(contract)", &project()).is_err());
    }

    #[test]
    fn an_empty_filter_is_refused_before_a_lane_asks_for_tools() {
        let lane = CiLaneConfig::default();
        assert!(validate(&lane, "branch", "  ", &project()).is_err());
        assert!(!validate(&lane, "branch", "test(contract)", &project()).unwrap());
    }

    #[test]
    fn unrelated_lane_steps_are_left_unchanged() {
        for (role, input) in [
            ("just", vec!["check"]),
            ("just", vec!["test", "ui"]),
            ("cargo", vec!["test", "run"]),
        ] {
            let mut args: Vec<String> = input.iter().map(|arg| (*arg).to_owned()).collect();
            let before = args.clone();
            assert!(!apply(role, &mut args, "test(contract)", &project()).unwrap());
            assert_eq!(args, before);
        }
    }
}
