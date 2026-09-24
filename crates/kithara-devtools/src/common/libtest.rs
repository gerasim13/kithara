/// The libtest arguments that run one `#[ignore]`d test of this binary.
///
/// A test needing a real child spawns the test binary itself and names an
/// ignored test inside it. `module_path!()` expands at its call site, so it
/// arrives here as an argument rather than being read here.
#[cfg(test)]
pub(crate) fn child_test_args(module: &str, name: &str) -> Vec<String> {
    let module = module.split_once("::").map_or(module, |(_, module)| module);
    vec![
        format!("{module}::{name}"),
        "--exact".to_owned(),
        "--ignored".to_owned(),
        "--nocapture".to_owned(),
    ]
}
