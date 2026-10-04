use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};

use super::request::TestRequest;
use crate::{
    common::project::{TestCommandConfig, TestLaneConfig},
    consts,
};

pub(super) fn validate_config(config: &TestCommandConfig) -> Result<()> {
    if config.default_lane.is_empty() {
        bail!("missing test.default_lane in .config/xtask.toml");
    }
    if config.default_backend.is_empty() {
        bail!("missing test.default_backend in .config/xtask.toml");
    }
    if config.feature_arg.is_empty() {
        bail!("missing test.feature_arg in .config/xtask.toml");
    }
    if config.nextest_config.is_empty() {
        bail!("missing test.nextest_config in .config/xtask.toml");
    }
    if !config.lanes.contains_key(&config.default_lane) {
        bail!(
            "test.default_lane `{}` is not defined in test.lanes",
            config.default_lane
        );
    }
    if !config.loom_lane.is_empty() && !config.lanes.contains_key(&config.loom_lane) {
        bail!(
            "test.loom_lane `{}` is not defined in test.lanes",
            config.loom_lane
        );
    }
    for (name, lane) in &config.lanes {
        for entry in &lane.undeclared_toggles {
            if entry != consts::FLASH_TOGGLE && entry != consts::NO_BLOCK_TOGGLE {
                bail!(
                    "test.lanes.{name}.undeclared_toggles carries `{entry}`; valid toggles are `{FLASH_TOGGLE}` and `{NO_BLOCK_TOGGLE}`",
                    FLASH_TOGGLE = consts::FLASH_TOGGLE,
                    NO_BLOCK_TOGGLE = consts::NO_BLOCK_TOGGLE
                );
            }
        }
    }
    if !config.net_backends.contains_key(&config.default_backend) {
        bail!(
            "test.default_backend `{}` is not defined in test.net_backends",
            config.default_backend
        );
    }
    for (name, lane) in &config.lanes {
        if lane.program.is_empty() {
            bail!("test.lanes.{name}.program is empty");
        }
        if let Some(backend) = &lane.default_backend
            && !config.net_backends.contains_key(backend)
        {
            bail!(
                "test.lanes.{name}.default_backend `{backend}` is not configured under test.net_backends"
            );
        }
        passthrough_position(lane).with_context(|| format!("test.lanes.{name}.passthrough"))?;
    }
    let mut named = BTreeSet::new();
    for flake in &config.known_flakes {
        if flake.test.is_empty() {
            bail!("test.known_flakes entry names no test");
        }
        if flake.issue.is_empty() {
            bail!(
                "test.known_flakes entry `{}` names no issue: an entry without an owner is a flake nobody removes",
                flake.test
            );
        }
        if !named.insert(flake.test.as_str()) {
            bail!("test.known_flakes names `{}` twice", flake.test);
        }
    }
    Ok(())
}

pub(super) fn select_lane<'a>(
    config: &'a TestCommandConfig,
    request: &'a TestRequest,
) -> Result<(&'a str, &'a TestLaneConfig)> {
    let explicit_lane = match request.lanes.as_slice() {
        [] => None,
        [lane] => Some(lane.as_str()),
        _ => bail!("more than one --lane needs --touched to choose between them"),
    };
    let lane_name = match request.loom {
        Some(true) => {
            if config.loom_lane.is_empty() {
                bail!("--loom=on requires test.loom_lane in .config/xtask.toml");
            }
            if let Some(explicit_lane) = explicit_lane
                && explicit_lane != config.loom_lane
            {
                bail!(
                    "--loom=on selects lane `{}` and conflicts with --lane={explicit_lane}",
                    config.loom_lane
                );
            }
            config.loom_lane.as_str()
        }
        Some(false)
            if explicit_lane == Some(config.loom_lane.as_str()) && !config.loom_lane.is_empty() =>
        {
            bail!("--loom=off conflicts with --lane={}", config.loom_lane);
        }
        Some(false) | None => explicit_lane.unwrap_or(&config.default_lane),
    };
    let Some(lane) = config.lanes.get(lane_name) else {
        let valid = config
            .lanes
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("unsupported test lane `{lane_name}`; configured values: {valid}");
    };
    Ok((lane_name, lane))
}

#[derive(Clone, Copy)]
pub(crate) struct LaneToggles {
    pub(crate) flash: bool,
    pub(crate) no_block: bool,
}

/// Resolve one toggle for a lane.
///
/// A lane lists a toggle in `undeclared_toggles` when none of its packages
/// declares that feature: the tools have no `no-block`, the UI crates have no
/// `flash`. Cargo applies an unqualified feature to every selected package and
/// fails the run when none of them declares it, so a run-wide request — the
/// gate asks every touched lane for the detector — must leave such a lane
/// alone. A lane that merely defaults a toggle off still honours the request.
pub(super) fn toggle(
    name: &str,
    requested: Option<bool>,
    lane_default: Option<bool>,
    lane: &TestLaneConfig,
    default: bool,
) -> bool {
    if lane.undeclared_toggles.iter().any(|entry| entry == name) {
        return false;
    }
    requested.unwrap_or_else(|| lane_default.unwrap_or(default))
}

pub(super) fn lane_toggles(
    config: &TestCommandConfig,
    lane: &TestLaneConfig,
    request: Option<&TestRequest>,
) -> LaneToggles {
    LaneToggles {
        flash: toggle(
            consts::FLASH_TOGGLE,
            request.and_then(|request| request.flash),
            lane.default_flash,
            lane,
            config.flash.default,
        ),
        no_block: toggle(
            consts::NO_BLOCK_TOGGLE,
            request.and_then(|request| request.no_block),
            lane.default_no_block,
            lane,
            config.no_block.default,
        ),
    }
}

pub(super) fn features_for(
    config: &TestCommandConfig,
    lane: &TestLaneConfig,
    request: &TestRequest,
) -> Result<BTreeSet<String>> {
    let backend = backend_name(config, lane, request);
    lane_features(
        config,
        lane,
        lane_toggles(config, lane, Some(request)),
        backend,
    )
}

pub(super) fn backend_name<'a>(
    config: &'a TestCommandConfig,
    lane: &'a TestLaneConfig,
    request: &'a TestRequest,
) -> &'a str {
    request
        .net_backend
        .as_deref()
        .or(lane.default_backend.as_deref())
        .unwrap_or(&config.default_backend)
}

pub(crate) fn lane_features(
    config: &TestCommandConfig,
    lane: &TestLaneConfig,
    toggles: LaneToggles,
    backend_name: &str,
) -> Result<BTreeSet<String>> {
    let mut features = BTreeSet::new();
    features.extend(config.features.iter().cloned());
    features.extend(lane.default_features.iter().cloned());
    if toggles.flash {
        features.extend(config.flash.features.iter().cloned());
    }
    if toggles.no_block {
        features.extend(config.no_block.features.iter().cloned());
    }
    let Some(backend) = config.net_backends.get(backend_name) else {
        let valid = config
            .net_backends
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("unsupported net backend `{backend_name}`; configured values: {valid}");
    };
    features.extend(backend.features.iter().cloned());
    Ok(features)
}

pub(super) enum PassthroughPosition {
    BeforeSuffix,
    AfterSuffix,
}

pub(super) fn passthrough_position(lane: &TestLaneConfig) -> Result<PassthroughPosition> {
    match lane.passthrough.as_str() {
        "" | "before-suffix" => Ok(PassthroughPosition::BeforeSuffix),
        "after-suffix" => Ok(PassthroughPosition::AfterSuffix),
        value => bail!("unsupported passthrough position `{value}`"),
    }
}
