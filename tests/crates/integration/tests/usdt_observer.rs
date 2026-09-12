#![cfg(target_os = "macos")]

use std::ffi::OsString;

use kithara::platform::time::Duration;
use kithara_integration_tests::{kithara, usdt_observer};

#[kithara::test(native, flash(false))]
fn dtrace_can_attach_to_a_same_user_child() {
    usdt_observer::preflight().expect("Apple USDT runner must allow same-user DTrace attach");
}

#[kithara::test(native, flash(false))]
fn parses_operation_and_all_payload_slots() {
    let records = usdt_observer::parse_records("noise\nKITHARA_USDT|probe_3|42|7|8|9\n")
        .expect("well-formed raw record must parse");
    assert_eq!(
        records,
        [usdt_observer::ProbeRecord {
            arity: 3,
            operation: 42,
            payload: [7, 8, 9, 0, 0],
        }]
    );
}

#[kithara::test(native, flash(false))]
fn rejects_an_incomplete_raw_record() {
    let error = usdt_observer::parse_records("KITHARA_USDT|probe_2|42|7\n")
        .expect_err("missing payload slot must be rejected");
    assert!(error.to_string().contains("expected operation id plus 2"));
}

#[kithara::test(native, flash(false))]
fn observer_uses_non_interactive_sudo_and_the_target_pid() {
    let command =
        usdt_observer::observer_command(1234).expect("default observation window must be valid");
    assert_eq!(command.program, "sudo");
    assert_eq!(command.args[0], OsString::from("-n"));
    assert_eq!(command.args[1], OsString::from("/usr/sbin/dtrace"));
    assert_eq!(command.args[5], OsString::from("-p"));
    assert_eq!(command.args[6], OsString::from("1234"));
}

#[kithara::test(native, flash(false))]
fn observer_exits_without_waiting_for_its_target() {
    let command =
        usdt_observer::observer_command(1234).expect("default observation window must be valid");
    assert!(
        command.args[4]
            .to_string_lossy()
            .contains("tick-2sec { exit(0); }")
    );
}

#[kithara::test(native, flash(false))]
fn observer_waits_for_dtrace_readiness_before_returning() {
    let command =
        usdt_observer::observer_command(1234).expect("default observation window must be valid");
    assert!(
        command.args[4]
            .to_string_lossy()
            .contains("BEGIN { printf(\"KITHARA_USDT_READY\\n\"); }")
    );
}

#[kithara::test(native, flash(false))]
fn observer_accepts_a_bounded_product_capture_window() {
    let command = usdt_observer::observer_command_for(1234, Duration::from_secs(120))
        .expect("two-minute product test capture window must be valid");
    assert!(
        command.args[4]
            .to_string_lossy()
            .contains("tick-120sec { exit(0); }")
    );
}

#[kithara::test(native, flash(false))]
fn observer_rejects_an_invalid_capture_window() {
    let error = usdt_observer::observer_command_for(1234, Duration::ZERO)
        .expect_err("zero-length capture must be rejected");
    assert!(error.to_string().contains("positive whole number"));
}

#[kithara::test(native, flash(false))]
fn preflight_attaches_without_enabling_the_kithara_provider() {
    let command = usdt_observer::preflight_command(1234);
    assert_eq!(command.args[4], OsString::from("BEGIN { exit(0); }"));
}
