use anyhow::{Context, Result, bail};

use crate::consts;

pub(super) fn reject_reuse(args: &[String]) -> Result<()> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let (flag, attached) = option(arg);
        if consts::NEXTEST_REUSE_OPTIONS.contains(&flag) {
            bail!("`{flag}` requires the validated prepared test operation");
        }
        if !attached
            && [
                consts::NEXTEST_BUILD_VALUE_OPTIONS,
                consts::NEXTEST_SHARED_VALUE_OPTIONS,
                consts::NEXTEST_SELECTION_VALUE_OPTIONS,
                consts::NEXTEST_RUN_VALUE_OPTIONS,
            ]
            .iter()
            .any(|options| options.contains(&flag))
        {
            args.next();
        }
    }
    Ok(())
}

/// Separate build inputs from selection/runtime options; shared configuration reaches both.
pub(super) fn split(args: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    reject_reuse(args)?;
    let mut build = Vec::new();
    let mut test = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            test.push(arg.clone());
            test.extend(args.cloned());
            break;
        }
        let (flag, attached) = option(arg);
        let (scope, valued) = classify(arg, flag)?;
        let value = if valued && !attached {
            Some(
                args.next()
                    .with_context(|| format!("`{arg}` needs a value"))?,
            )
        } else {
            None
        };
        let values = std::iter::once(arg).chain(value).cloned();
        match scope {
            Scope::Build => build.extend(values),
            Scope::Test => test.extend(values),
            Scope::Both => {
                build.extend(values.clone());
                test.extend(values);
            }
        }
    }
    Ok((build, test))
}

#[derive(Clone, Copy)]
enum Scope {
    Build,
    Test,
    Both,
}

fn classify(arg: &str, flag: &str) -> Result<(Scope, bool)> {
    let options = [
        (Scope::Build, true, consts::NEXTEST_BUILD_VALUE_OPTIONS),
        (Scope::Build, false, consts::NEXTEST_BUILD_FLAGS),
        (Scope::Both, true, consts::NEXTEST_SHARED_VALUE_OPTIONS),
        (Scope::Both, false, consts::NEXTEST_SHARED_FLAGS),
        (Scope::Test, true, consts::NEXTEST_SELECTION_VALUE_OPTIONS),
        (Scope::Test, false, consts::NEXTEST_SELECTION_FLAGS),
        (Scope::Test, true, consts::NEXTEST_RUN_VALUE_OPTIONS),
        (Scope::Test, false, consts::NEXTEST_RUN_FLAGS),
    ];
    if let Some((scope, valued, _)) = options
        .iter()
        .find(|(_, _, options)| options.contains(&flag))
    {
        return Ok((*scope, *valued));
    }
    if arg
        .strip_prefix("-P")
        .is_some_and(|value| !value.is_empty())
    {
        return Ok((Scope::Both, false));
    }
    if !arg.starts_with('-')
        || ["-E", "-j"].iter().any(|prefix| {
            arg.strip_prefix(prefix)
                .is_some_and(|value| !value.is_empty())
        })
    {
        return Ok((Scope::Test, false));
    }
    bail!("unsupported prepared nextest option `{arg}`")
}

fn option(arg: &str) -> (&str, bool) {
    arg.split_once('=')
        .map_or((arg, false), |(flag, _)| (flag, true))
}
