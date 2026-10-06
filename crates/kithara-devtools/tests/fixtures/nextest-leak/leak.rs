use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[test]
fn leaks_output() {
    let child = Command::new(env::current_exe().expect("test binary"))
        .args(["--exact", "holds_output", "--ignored", "--nocapture"])
        .spawn()
        .expect("spawn output holder");
    let started = env::var_os("LEAK_STARTED_PATH").expect("started marker");
    fs::write(started, child.id().to_string()).expect("record output holder");
    drop(child);
}

#[test]
#[ignore = "subprocess entrypoint; run: just test run --lane=tooling -E 'test(leaked_output_fails_the_lane_and_stress_report)'"]
fn holds_output() {
    let release = PathBuf::from(env::var_os("LEAK_RELEASE_PATH").expect("release marker"));
    let released = PathBuf::from(env::var_os("LEAK_RELEASED_PATH").expect("released marker"));
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if release.exists() {
            fs::write(released, "released").expect("acknowledge output release");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}
