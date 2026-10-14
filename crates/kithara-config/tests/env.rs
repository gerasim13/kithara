#![cfg(not(target_arch = "wasm32"))]

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
use std::{env, ffi::OsString, path::PathBuf, process::Command, thread};

use kithara_config::{Config, path_from_env};
use kithara_test_utils::kithara;

const PATH_VARIABLE: &str = "KITHARA_CONFIG_ENV_PATH_TEST";
const CHILD_VARIABLE: &str = "KITHARA_CONFIG_ENV_PATH_CHILD";

#[derive(Config)]
#[config(fields(value, get(ref)))]
struct EnvironmentSettings {
    #[config(builder(default = path_from_env(PATH_VARIABLE)
        .unwrap_or_else(|| PathBuf::from("domain-default"))))]
    path: PathBuf,
}

#[kithara::test(native, flash(false))]
#[case::unset(None)]
#[case::empty(Some(OsString::new()))]
#[case::path(Some(OsString::from("configured/runtime")))]
#[case::native_path(Some(native_path()))]
fn an_environment_source_constructs_retained_settings(#[case] value: Option<OsString>) {
    if env::var_os(CHILD_VARIABLE).is_none() {
        let current_thread = thread::current();
        let name = current_thread.name().expect("harness test name");
        let mut child = Command::new(env::current_exe().expect("test executable"));
        child
            .arg("--exact")
            .arg(name)
            .arg("--nocapture")
            .env(CHILD_VARIABLE, "1")
            .env_remove(PATH_VARIABLE);
        if let Some(value) = &value {
            child.env(PATH_VARIABLE, value);
        }
        let output = child.output().expect("isolated environment source test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "isolated configuration failed: stdout={} stderr={}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "child executed the contract"
        );
        return;
    }

    let expected = value
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("domain-default"), PathBuf::from);
    let settings = EnvironmentSettings::builder().build();
    assert_eq!(settings.path(), &expected);
    assert_eq!(settings.values().path, expected);

    let explicit = PathBuf::from("explicit/runtime");
    let settings = EnvironmentSettings::builder()
        .path(explicit.clone())
        .build();
    assert_eq!(settings.path(), &explicit);
    assert_eq!(settings.values().path, explicit);
}

#[cfg(unix)]
fn native_path() -> OsString {
    OsString::from_vec(vec![b'r', b'u', b'n', b'-', 0xff])
}

#[cfg(windows)]
fn native_path() -> OsString {
    OsString::from_wide(&[0x72, 0x75, 0x6e, 0x2d, 0xd800])
}
