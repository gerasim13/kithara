//! The default campaign repeats every test some lane runs, or the lane says
//! why it is left out.

use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

use cargo_metadata::Metadata;

use crate::{
    common::project::{ProjectConfig, TestCargoOptions, TestLaneConfig, TestRunner},
    test::{
        ResolvedLane,
        repository_tests::{root, selected_by, this_workspace},
        resolve, toggled,
    },
};

/// Each package `cargo tree` resolves, with every feature set it builds the
/// package with.
type Build = BTreeMap<String, Vec<BTreeSet<String>>>;

/// The packages and features cargo resolves for a lane's build, read from
/// `cargo tree` over the lane's own selection and features.
fn build_of(lane: &ResolvedLane) -> Build {
    let cargo = std::env::var_os("CARGO").expect("the test runner names the cargo it uses");
    let mut command = Command::new(cargo);
    command.current_dir(root()).args([
        "tree",
        "--prefix",
        "none",
        "--format",
        "{p}|{f}",
        "--edges",
        "normal,build,dev",
    ]);
    if lane.cargo.workspace {
        command.arg("--workspace");
        for package in &lane.cargo.exclude {
            command.args(["--exclude", package]);
        }
    }
    for package in &lane.cargo.packages {
        command.args(["--package", package]);
    }
    if !lane.features.is_empty() {
        command.args(["--features", &lane.features.join(",")]);
    }
    let output = command.output().expect("run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree for lane `{}`: {}",
        lane.lane,
        String::from_utf8_lossy(&output.stderr)
    );
    let mut build = Build::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.strip_suffix(" (*)").unwrap_or(line);
        if let Some((package, features)) = line.split_once('|') {
            let features = features
                .split(',')
                .filter(|feature| !feature.is_empty())
                .map(str::to_owned)
                .collect();
            build.entry(package.to_owned()).or_default().push(features);
        }
    }
    build
}

/// Every package `own` builds, `theirs` builds too, with at least the same
/// features.
fn covers(own: &Build, theirs: &Build) -> bool {
    own.iter().all(|(package, sets)| {
        theirs.get(package).is_some_and(|more| {
            sets.iter()
                .all(|features| more.iter().any(|other| features.is_subset(other)))
        })
    })
}

/// `theirs` builds every target `own` builds: a lane narrowed to its library
/// or to named test targets runs part of what an unnarrowed one runs.
fn targets_within(own: &TestCargoOptions, theirs: &TestCargoOptions) -> bool {
    let narrows = |cargo: &TestCargoOptions| cargo.lib || !cargo.tests.is_empty();
    !narrows(theirs)
        || (narrows(own)
            && (!own.lib || theirs.lib)
            && own.tests.iter().all(|test| theirs.tests.contains(test)))
}

/// Whether `stressed` runs every test `lane` runs, features aside: both under
/// nextest at the campaign's thread count, the same Cargo profile, at least
/// the same environment, and a selection and filter no narrower.
fn runs_within(lane: &TestLaneConfig, stressed: &TestLaneConfig, metadata: &Metadata) -> bool {
    let (TestRunner::Nextest(own), TestRunner::Nextest(theirs)) = (&lane.runner, &stressed.runner)
    else {
        return false;
    };
    own.test_threads.is_none()
        && theirs.filter.is_none()
        && (theirs.ignore_default_filter || !own.ignore_default_filter)
        && lane.cargo.profile == stressed.cargo.profile
        && lane
            .env
            .iter()
            .all(|(key, value)| stressed.env.get(key) == Some(value))
        && selected_by(&lane.cargo, metadata).is_subset(&selected_by(&stressed.cargo, metadata))
        && targets_within(&lane.cargo, &stressed.cargo)
}

/// The builds of `lanes`, resolved side by side: each `cargo tree` waits on
/// the resolver for seconds, and the rule reads dozens.
fn builds_of(lanes: &[ResolvedLane]) -> Vec<Build> {
    let workers = std::thread::available_parallelism().map_or(1, usize::from);
    std::thread::scope(|scope| {
        let handles = lanes
            .chunks(lanes.len().div_ceil(workers).max(1))
            .map(|chunk| scope.spawn(move || chunk.iter().map(build_of).collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a cargo tree worker finishes"))
            .collect()
    })
}

/// Every lane is repeated by the default campaign or says why not. The
/// campaign repeats a lane it names, and a lane whose every test a named lane
/// already runs in a build with at least its features: naming that one would
/// repeat the same tests twice, and exempting it would report a gap that is
/// not there.
#[test]
fn every_lane_is_stressed_exempt_or_covered() {
    let project = ProjectConfig::load(&root()).expect("load repository config");
    let metadata = this_workspace();
    let (test, stress) = (&project.test, &project.stress);
    let resolved = |lane: &str, flash: Option<bool>, no_block: Option<bool>| {
        resolve(test, &toggled(test, lane, flash, no_block).expect("lane")).expect("resolve")
    };
    let mut failures = Vec::new();
    if stress.default_filter != "all()" {
        failures.push(format!(
            "stress.default_filter `{}` leaves part of every stressed lane unrepeated",
            stress.default_filter
        ));
    }
    let candidates = test
        .lanes
        .iter()
        .filter(|(name, _)| !stress.lanes.contains(name))
        .map(|(name, lane)| {
            let within = stress
                .lanes
                .iter()
                .filter(|stressed| runs_within(lane, &test.lanes[stressed.as_str()], &metadata))
                .collect::<Vec<_>>();
            (name, within)
        })
        .collect::<Vec<_>>();
    let mut lanes = Vec::new();
    let mut own = BTreeMap::new();
    for (name, within) in &candidates {
        if !within.is_empty() {
            own.insert(name.as_str(), lanes.len());
            lanes.push(resolved(name, None, None));
        }
    }
    let mut units = BTreeMap::<&str, Vec<usize>>::new();
    for stressed in candidates.iter().flat_map(|(_, within)| within) {
        if units.contains_key(stressed.as_str()) {
            continue;
        }
        let mut indices = Vec::new();
        for name in &stress.default_modes {
            let mode = &stress.modes[name];
            if mode.command.is_empty() {
                indices.push(lanes.len());
                lanes.push(resolved(stressed, mode.flash, mode.no_block));
            }
        }
        units.insert(stressed.as_str(), indices);
    }
    let builds = builds_of(&lanes);
    for (name, within) in &candidates {
        let covered_by = within.iter().find(|stressed| {
            let lane = &builds[own[name.as_str()]];
            units[stressed.as_str()]
                .iter()
                .any(|&unit| covers(lane, &builds[unit]))
        });
        match (covered_by, stress.not_stressed.contains_key(name.as_str())) {
            (Some(stressed), true) => failures.push(format!(
                "lane `{name}` is exempt from stress, but stress lane `{stressed}` already repeats \
                 every test it runs"
            )),
            (None, false) => failures.push(format!(
                "lane `{name}` is not in stress.lanes, has no reason in stress.not_stressed, and no \
                 stress lane runs every test it runs"
            )),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
