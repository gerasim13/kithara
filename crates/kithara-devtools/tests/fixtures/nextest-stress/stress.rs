#[test]
fn an_earlier_iteration_fails() {
    assert_eq!(
        std::env::var("NEXTEST_STRESS_CURRENT").expect("nextest stress iteration"),
        "1",
        "the first iteration fails and the final iteration passes"
    );
}
