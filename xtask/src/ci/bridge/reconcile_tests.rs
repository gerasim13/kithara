use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use super::*;

#[derive(Debug, Eq, PartialEq)]
enum ImportAction {
    CheckProvenance,
    RefreshHeads,
    Push(String),
}

#[derive(Debug, Eq, PartialEq)]
enum VerificationAction {
    Create,
    Attach(u64, u64),
    Announce(u64, u64),
    Observe(u64),
    Report(String, String),
    Finish(VerificationState),
    Reject(u64),
}

#[test]
fn main_reconciliation_precedes_verification_and_fast_forward_skips_it() {
    let actions = RefCell::new(Vec::new());
    reconcile_main_first(
        || {
            actions.borrow_mut().push("main");
            Ok(None)
        },
        |_| {
            actions.borrow_mut().push("verification");
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(*actions.borrow(), ["main"]);
}

#[test]
fn equal_main_runs_verification_after_reconciliation() {
    let actions = RefCell::new(Vec::new());
    reconcile_main_first(
        || {
            actions.borrow_mut().push("main");
            Ok(Some("base".into()))
        },
        |base| {
            assert_eq!(base, "base");
            actions.borrow_mut().push("verification");
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(*actions.borrow(), ["main", "verification"]);
}

#[test]
fn verifier_errors_do_not_fail_completed_main_reconciliation() {
    reconcile_main_first(
        || Ok(Some("base".into())),
        |_| bail!("transient GitHub error"),
    )
    .unwrap();
}

#[test]
fn one_state_directory_serializes_all_reconciliation_keys() {
    let directory = tempfile::tempdir().unwrap();
    let first = ReconcileLock::acquire(directory.path()).unwrap();

    assert!(
        ReconcileLock::try_acquire(directory.path())
            .unwrap()
            .is_none()
    );
    drop(first);
    assert!(
        ReconcileLock::try_acquire(directory.path())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_new_verification_attaches_then_posts_to_exact_head_without_observing() {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let actions = RefCell::new(Vec::new());
    start_verification(
        head,
        3,
        &[],
        || {
            actions.borrow_mut().push(VerificationAction::Create);
            Ok(42)
        },
        |attempt, id| {
            actions
                .borrow_mut()
                .push(VerificationAction::Attach(attempt, id));
            Ok(())
        },
        |sha, state, _| {
            assert_eq!(sha, head);
            actions
                .borrow_mut()
                .push(VerificationAction::Report(sha.into(), state.into()));
            Ok(())
        },
        |attempt, id| {
            actions
                .borrow_mut()
                .push(VerificationAction::Announce(attempt, id));
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        *actions.borrow(),
        [
            VerificationAction::Create,
            VerificationAction::Attach(3, 42),
            VerificationAction::Report(head.into(), "pending".into()),
            VerificationAction::Announce(3, 42),
        ]
    );
}

#[test]
fn one_later_tick_observes_once_and_running_keeps_testing() {
    let calls = RefCell::new(0);
    observe_verification(
        "head",
        42,
        |id| {
            assert_eq!(id, 42);
            *calls.borrow_mut() += 1;
            Ok(PipelineObservation::Running)
        },
        |_, _, _| panic!("running is not a new commit-status transition"),
        |_, _, _| panic!("running must stay Testing"),
    )
    .unwrap();
    assert_eq!(*calls.borrow(), 1);
}

#[test]
fn success_is_posted_before_verified() {
    let actions = RefCell::new(Vec::new());
    observe_verification(
        "head",
        42,
        |id| {
            actions.borrow_mut().push(VerificationAction::Observe(id));
            Ok(PipelineObservation::Succeeded)
        },
        |sha, state, _| {
            actions
                .borrow_mut()
                .push(VerificationAction::Report(sha.into(), state.into()));
            Ok(())
        },
        |_, state, _| {
            actions.borrow_mut().push(VerificationAction::Finish(state));
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        *actions.borrow(),
        [
            VerificationAction::Observe(42),
            VerificationAction::Report("head".into(), "success".into()),
            VerificationAction::Finish(VerificationState::Verified),
        ]
    );
}

#[test]
fn failed_and_invalid_proof_post_a_verdict_before_rejection() {
    for (observation, github_state) in [
        (PipelineObservation::Failed("failed".into()), "failure"),
        (
            PipelineObservation::Invalid("missing child".into()),
            "error",
        ),
    ] {
        let actions = RefCell::new(Vec::new());
        observe_verification(
            "head",
            42,
            |_| Ok(observation.clone()),
            |_, state, _| {
                actions
                    .borrow_mut()
                    .push(VerificationAction::Report("head".into(), state.into()));
                Ok(())
            },
            |_, state, _| {
                actions.borrow_mut().push(VerificationAction::Finish(state));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            *actions.borrow(),
            [
                VerificationAction::Report("head".into(), github_state.into()),
                VerificationAction::Finish(VerificationState::Rejected),
            ]
        );
    }
}

#[test]
fn transient_observation_errors_do_not_manufacture_rejection() {
    let error = observe_verification(
        "head",
        42,
        |_| bail!("temporary GitLab API failure"),
        |_, _, _| panic!("an API error is not a verdict"),
        |_, _, _| panic!("an API error must leave Testing unchanged"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("temporary GitLab API failure"));
}

#[test]
fn a_lost_create_response_is_recovered_without_a_second_pipeline() {
    let pipelines = RefCell::new(Vec::new());
    let creates = Cell::new(0);
    let first_discovery = pipelines.borrow().clone();
    let first = recover_or_create(&first_discovery, || {
        creates.set(creates.get() + 1);
        pipelines.borrow_mut().push(42);
        bail!("pipeline was created but its response was lost")
    });
    assert!(first.is_err());

    let second_discovery = pipelines.borrow().clone();
    let recovered = recover_or_create(&second_discovery, || {
        panic!("recovery must not create a second pipeline")
    })
    .unwrap();

    assert_eq!(recovered, 42);
    assert_eq!(creates.get(), 1);
}

#[test]
fn ambiguous_recovery_fails_closed_without_creating() {
    let error = recover_or_create(&[41, 42], || panic!("ambiguity must not create")).unwrap_err();
    assert!(error.to_string().contains("multiple pipelines"));
}

#[test]
fn an_attempt_ref_changes_only_when_retry_advances_the_generation() {
    assert_eq!(
        quarantine_ref("head", "base", 1),
        "quarantine/github/head/base/attempt-1"
    );
    assert_eq!(
        quarantine_ref("head", "base", 2),
        "quarantine/github/head/base/attempt-2"
    );
}

#[test]
fn control_path_changes_post_failure_and_reject_without_a_pipeline() {
    let directory = tempfile::tempdir().unwrap();
    let ledger = Ledger::new(directory.path()).unwrap();
    let entry = ledger.reserve("head", "base").unwrap();
    let actions = RefCell::new(Vec::new());

    let handled = reject_control_changes(
        427,
        "head",
        &entry,
        &[".gitlab-ci.yml".into()],
        |sha, state, _| {
            actions
                .borrow_mut()
                .push(VerificationAction::Report(sha.into(), state.into()));
            Ok(())
        },
        |attempt, detail| {
            actions
                .borrow_mut()
                .push(VerificationAction::Reject(attempt));
            ledger.reject("head", "base", attempt, detail)
        },
    )
    .unwrap();

    assert!(handled);
    assert_eq!(
        *actions.borrow(),
        [
            VerificationAction::Report("head".into(), "failure".into()),
            VerificationAction::Reject(1),
        ]
    );
    let rejected = ledger.get("head", "base").unwrap().unwrap();
    assert_eq!(rejected.state, VerificationState::Rejected);
    assert_eq!(rejected.pipeline_id, None);
}

#[test]
fn product_only_changes_continue_to_quarantine() {
    let directory = tempfile::tempdir().unwrap();
    let ledger = Ledger::new(directory.path()).unwrap();
    let entry = ledger.reserve("head", "base").unwrap();

    assert!(
        !reject_control_changes(
            427,
            "head",
            &entry,
            &[],
            |_, _, _| panic!("product-only changes must not report a policy failure"),
            |_, _| panic!("product-only changes must not be rejected"),
        )
        .unwrap()
    );
}

#[test]
fn a_merged_github_head_is_only_fast_forwarded() {
    let github_sha = "0123456789abcdef0123456789abcdef01234567";
    let gitlab_sha = "89abcdef0123456789abcdef0123456789abcdef";
    let actions = Rc::new(RefCell::new(Vec::new()));

    fast_forward_github_import(
        github_sha,
        gitlab_sha,
        "main",
        {
            let actions = Rc::clone(&actions);
            move |_| {
                actions.borrow_mut().push(ImportAction::CheckProvenance);
                Ok(Some(427))
            }
        },
        {
            let actions = Rc::clone(&actions);
            move || {
                actions.borrow_mut().push(ImportAction::RefreshHeads);
                Ok((github_sha.to_owned(), gitlab_sha.to_owned()))
            }
        },
        |_| panic!("a merged pull request must not open an incident"),
        {
            let actions = Rc::clone(&actions);
            move |sha, branch| {
                actions
                    .borrow_mut()
                    .push(ImportAction::Push(format!("{sha}:{branch}")));
                Ok(())
            }
        },
    )
    .unwrap();

    assert_eq!(
        *actions.borrow(),
        [
            ImportAction::CheckProvenance,
            ImportAction::RefreshHeads,
            ImportAction::Push(format!("{github_sha}:main")),
        ]
    );
}

#[test]
fn changed_heads_stop_the_import_before_the_push() {
    let github_sha = "0123456789abcdef0123456789abcdef01234567";
    let gitlab_sha = "89abcdef0123456789abcdef0123456789abcdef";

    let error = fast_forward_github_import(
        github_sha,
        gitlab_sha,
        "main",
        |_| Ok(Some(427)),
        || {
            Ok((
                "fedcba9876543210fedcba9876543210fedcba98".into(),
                gitlab_sha.into(),
            ))
        },
        |_| panic!("a merged pull request must not open an incident"),
        |_, _| panic!("a stale observation must not be pushed"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("heads changed"));
}

#[test]
fn a_direct_github_update_opens_an_incident_without_a_push() {
    let github_sha = "0123456789abcdef0123456789abcdef01234567";
    let detail = Rc::new(RefCell::new(None));

    let error = fast_forward_github_import(
        github_sha,
        "89abcdef0123456789abcdef0123456789abcdef",
        "main",
        |_| Ok(None),
        || panic!("an untrusted head must not refresh for promotion"),
        {
            let detail = Rc::clone(&detail);
            move |message| {
                *detail.borrow_mut() = Some(message.to_owned());
                Ok(())
            }
        },
        |_, _| panic!("an untrusted head must not be pushed"),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("not associated with a merged pull request")
    );
    assert_eq!(
        detail.borrow().as_deref(),
        Some(
            "GitHub head 0123456789abcdef0123456789abcdef01234567 is not associated with a \
             merged pull request targeting main"
        )
    );
}

/// Trust says who may change the CI configuration. It does not say which
/// configuration the verification runs, and conflating the two took the
/// normalization away from every trusted branch at once: a June branch went to
/// the host with its own `xtask`, which could not parse the host profile, and
/// its own pipeline, which still named a job the default branch had dropped.
/// Both died before a single test.
#[test]
fn only_a_pull_request_that_changes_the_controls_is_judged_with_its_own() {
    assert!(
        judged_with_own_controls(true, true),
        "a trusted author's control-path change has nowhere else to be tested"
    );
    assert!(
        !judged_with_own_controls(true, false),
        "a trusted author's stale branch needs the base's controls like any other"
    );
    assert!(!judged_with_own_controls(false, true));
    assert!(!judged_with_own_controls(false, false));
}
