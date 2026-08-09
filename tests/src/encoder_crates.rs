#[cfg(test)]
use std::collections::BTreeSet;

#[cfg(test)]
use toml::{Table, Value};

/// The crates whose resolved build determines an encoded byte. Read from the
/// lockfile by name, so a version bump to any of them still lands in a fresh
/// namespace.
pub(crate) const ENCODERS: &[&str] = &["fdk-aac", "fdk-aac-sys", "ffmpeg-next", "ffmpeg-sys-next"];

// Keeping byte-neutral crates separate makes every addition an explicit
// cache-invalidation decision.
#[cfg(test)]
pub(crate) const NON_ENCODING_DEPENDENCIES: &[&str] = &["tempfile", "thiserror", "tracing"];

#[cfg(test)]
fn package_for_dependency<'a>(packages: &'a [Value], dependency: &str) -> Option<&'a Value> {
    let mut fields = dependency.split_whitespace();
    let name = fields.next()?;
    let version = fields.next();
    let source = fields
        .next()
        .map(|source| source.trim_matches(&['(', ')'][..]));

    packages.iter().find(|package| {
        package.get("name").and_then(Value::as_str) == Some(name)
            && version.is_none_or(|version| {
                package.get("version").and_then(Value::as_str) == Some(version)
            })
            && source
                .is_none_or(|source| package.get("source").and_then(Value::as_str) == Some(source))
    })
}

#[cfg(test)]
pub(crate) fn unaccounted_direct_external_dependencies(
    lockfile: &str,
) -> Result<BTreeSet<String>, &'static str> {
    let lock: Table = lockfile.parse().map_err(|_| "lockfile must be TOML")?;
    let packages = lock
        .get("package")
        .and_then(Value::as_array)
        .ok_or("lockfile must list packages")?;
    let encoder = packages
        .iter()
        .find(|package| {
            package.get("name").and_then(Value::as_str) == Some("kithara-encode")
                && package.get("source").is_none()
        })
        .ok_or("lockfile must list the workspace kithara-encode package")?;
    let dependencies = encoder
        .get("dependencies")
        .and_then(Value::as_array)
        .ok_or("kithara-encode must list dependencies")?;

    dependencies
        .iter()
        .try_fold(BTreeSet::new(), |mut unaccounted, dependency| {
            let dependency = dependency
                .as_str()
                .ok_or("package dependency must be a string")?;
            let package = package_for_dependency(packages, dependency)
                .ok_or("direct dependency must resolve to a package")?;
            let name = package
                .get("name")
                .and_then(Value::as_str)
                .ok_or("package must have a name")?;

            if package.get("source").is_some()
                && !ENCODERS.contains(&name)
                && !NON_ENCODING_DEPENDENCIES.contains(&name)
            {
                unaccounted.insert(name.to_owned());
            }

            Ok(unaccounted)
        })
}

#[cfg(test)]
mod tests {
    use super::{BTreeSet, unaccounted_direct_external_dependencies};

    const WORKSPACE_LOCKFILE: &str = include_str!("../../Cargo.lock");
    const UNCLASSIFIED_DIRECT_DEPENDENCY: &str = r#"
version = 4

[[package]]
name = "kithara-encode"
version = "0.0.1"
dependencies = ["mp3lame-sys 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)"]

[[package]]
name = "mp3lame-sys"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    const CLASSIFIED_ENCODER_DEPENDENCY: &str = r#"
version = 4

[[package]]
name = "kithara-encode"
version = "0.0.1"
dependencies = ["fdk-aac 0.7.0"]

[[package]]
name = "fdk-aac"
version = "0.7.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    const TRANSITIVE_DEPENDENCY_OF_LOCAL_PACKAGE: &str = r#"
version = 4

[[package]]
name = "kithara-encode"
version = "0.0.1"
dependencies = ["kithara-workspace-hack"]

[[package]]
name = "kithara-workspace-hack"
version = "0.0.0"
dependencies = ["mp3lame-sys"]

[[package]]
name = "mp3lame-sys"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    fn unaccounted(lockfile: &str) -> BTreeSet<String> {
        unaccounted_direct_external_dependencies(lockfile)
            .expect("test lockfile must describe kithara-encode dependencies")
    }

    #[test]
    fn direct_external_dependencies_are_classified() {
        let unaccounted = unaccounted(WORKSPACE_LOCKFILE);

        assert!(
            unaccounted.is_empty(),
            "unclassified direct external kithara-encode dependencies: {unaccounted:?}; add each \
             crate to ENCODERS if it can change encoded bytes, otherwise to \
             NON_ENCODING_DEPENDENCIES"
        );
    }

    #[test]
    fn unclassified_direct_external_dependency_is_reported() {
        let unaccounted = unaccounted(UNCLASSIFIED_DIRECT_DEPENDENCY);

        assert_eq!(unaccounted, BTreeSet::from(["mp3lame-sys".to_owned()]));
    }

    #[test]
    fn classified_encoder_dependency_is_not_reported() {
        let unaccounted = unaccounted(CLASSIFIED_ENCODER_DEPENDENCY);

        assert!(unaccounted.is_empty());
    }

    #[test]
    fn dependency_through_local_package_is_not_reported() {
        let unaccounted = unaccounted(TRANSITIVE_DEPENDENCY_OF_LOCAL_PACKAGE);

        assert!(unaccounted.is_empty());
    }
}
