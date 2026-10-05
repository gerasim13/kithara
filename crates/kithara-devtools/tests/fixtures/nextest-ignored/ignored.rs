use std::{env, fs};

#[test]
fn ordinary_pass() {}

#[test]
#[ignore = "issue: https://github.com/zvuk/kithara/issues/548; nightly: red"]
fn pinned_red() {
    panic!("the report fixture pins an expected failure");
}

#[test]
#[ignore = "issue: https://github.com/zvuk/kithara/issues/548; nightly: flake"]
fn declared_flake() {
    let attempt = env::var_os("IGNORED_ATTEMPT_PATH").expect("attempt marker");
    if fs::read(&attempt).is_err() {
        fs::write(attempt, "first attempt").expect("record first attempt");
        panic!("the report fixture fails exactly one of two attempts");
    }
}

#[test]
#[ignore = "lane: tooling; manual fixture entrypoint excluded from the nightly selection"]
fn manual_entrypoint() {
    panic!("the manual fixture must not run in the selected audit");
}
