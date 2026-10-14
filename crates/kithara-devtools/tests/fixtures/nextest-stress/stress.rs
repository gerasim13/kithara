#[test]
fn an_earlier_iteration_fails() {
    let cargo = std::env::var_os("CARGO").expect("the runner supplies its Cargo executable");
    let version = std::process::Command::new(cargo)
        .arg("--version")
        .output()
        .expect("run the supplied Cargo executable");
    assert!(
        version.status.success() && version.stdout.starts_with(b"cargo "),
        "the runner must supply a working Cargo: {version:?}"
    );
    assert_eq!(
        std::env::var("NEXTEST_STRESS_CURRENT").expect("nextest stress iteration"),
        "1",
        "the first iteration fails and the final iteration passes"
    );
}
