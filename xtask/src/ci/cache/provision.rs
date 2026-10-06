use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use tracing::info;

use super::{client::provisioned_environment, required};
use crate::{
    child::{self, Cancel},
    ci::host::mac::{read_secret, write_secure},
    consts,
};

fn secret(path: &Path) -> Result<String> {
    let mut bytes = [0; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let value = hex::encode(bytes);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(value.as_bytes())?;
            Ok(value)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => read_secret(path),
        Err(error) => Err(error).context("create cache credential"),
    }
}

pub(super) fn credentials() -> Result<()> {
    let root = Path::new("/config");
    fs::create_dir_all(root)?;
    if !root.join("admin-user").exists() {
        write_secure(&root.join("admin-user"), "kithara-cache-admin")?;
    }
    secret(&root.join("admin-password"))?;
    Ok(())
}

/// Runs one administration step; a stop signal ends it and the setup.
fn rc(arguments: &[&str], cancel: &Cancel) -> Result<()> {
    let status = child::run(
        Command::new("rc")
            .args(arguments)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        Some(cancel),
    )
    .context("cache administration client")?;
    // Arguments and client diagnostics can include credentials.
    ensure!(
        status.success(),
        "cache administration operation failed: {status}"
    );
    Ok(())
}

/// The bytes a scope's bucket is allowed, which is not one number for the
/// fleet.
///
/// The scopes hold different things: the trusted one carries the layers every
/// job restores, a review scope carries whatever the branches under it happen
/// to publish. They were sized apart on the live host by hand, and a single
/// `CACHE_BUCKET_QUOTA` meant the next initialize would flatten them back to
/// one value - measured as 200 and 800 gibibytes standing against an
/// environment that still said 50. A scope may name its own, and the shared
/// value is what a scope that does not is given.
pub(super) fn scope_quota(scope: &str, shared: &str) -> Result<u64> {
    let named = format!(
        "CACHE_BUCKET_QUOTA_{}",
        scope.to_ascii_uppercase().replace('-', "_")
    );
    let value = env::var(&named).unwrap_or_else(|_| shared.to_owned());
    size(&value).with_context(|| format!("the quota of the {scope} scope"))
}

/// A size as the host writes it. The store is handed the bytes, so its quota
/// and the evictor's budget cannot read one size two ways.
fn size(value: &str) -> Result<u64> {
    let count = value.trim_end_matches(|character: char| character.is_ascii_alphabetic());
    let shift = match &value[count.len()..] {
        "KiB" => 10,
        "MiB" => 20,
        "GiB" => 30,
        "TiB" => 40,
        _ => bail!("{value} is not a whole number of KiB, MiB, GiB or TiB"),
    };
    ensure!(
        !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()),
        "{value} is not a whole number of KiB, MiB, GiB or TiB"
    );
    let bytes = count
        .parse::<u64>()?
        .checked_mul(1 << shift)
        .with_context(|| format!("{value} does not fit in 64 bits"))?;
    // The store reads zero as no quota at all.
    ensure!(bytes > 0, "a quota of {value} leaves the bucket no room");
    Ok(bytes)
}

pub(super) fn scope_bucket(scope: &str) -> Result<String> {
    ensure!(
        !scope.is_empty()
            && scope.len() <= 48
            && scope.ends_with(|character: char| character.is_ascii_alphanumeric())
            && scope
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "cache scope must contain 1..48 lowercase letters, digits or hyphens and end in a letter or digit"
    );
    Ok(format!("kithara-{scope}"))
}

pub(super) fn initialize(cancel: &Cancel) -> Result<()> {
    let scopes = required("CACHE_SCOPES")?;
    let quota = required("CACHE_BUCKET_QUOTA")?;
    let endpoint = required("CACHE_CLIENT_ENDPOINT")?;
    let uid = required("CACHE_CLIENT_UID")?.parse::<u32>()?;
    let url = reqwest::Url::parse(&endpoint)?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && !endpoint.chars().any(char::is_whitespace),
        "cache endpoint must be an HTTP URL without whitespace"
    );
    let scopes = scopes
        .split_whitespace()
        .map(|scope| {
            scope_bucket(scope)?;
            Ok((scope, scope_quota(scope, &quota)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let root = Path::new("/config");
    rc(
        &[
            "alias",
            "set",
            "--",
            "ci",
            consts::CACHE_STORE_URL,
            &read_secret(&root.join("admin-user"))?,
            &read_secret(&root.join("admin-password"))?,
        ],
        cancel,
    )?;
    rc(
        &[
            "bucket",
            "create",
            "--ignore-existing",
            &format!("ci/{}", consts::RECENCY_BUCKET),
        ],
        cancel,
    )?;
    for (scope, quota) in scopes {
        initialize_scope(scope, quota, &endpoint, uid, cancel)?;
    }
    Ok(())
}

fn initialize_scope(
    scope: &str,
    quota: u64,
    endpoint: &str,
    uid: u32,
    cancel: &Cancel,
) -> Result<()> {
    let bucket = scope_bucket(scope)?;
    let destination = format!("ci/{bucket}");
    let directory = Path::new("/clients").join(scope);
    fs::create_dir_all(&directory)?;
    let key = secret(&directory.join("access-key"))?;
    let password = secret(&directory.join("secret-key"))?;
    rc(
        &["bucket", "create", "--ignore-existing", &destination],
        cancel,
    )?;
    rc(
        &["bucket", "quota", "set", &destination, &quota.to_string()],
        cancel,
    )?;
    let mut lifecycle = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(&mut lifecycle, &retention())?;
    rc(
        &[
            "bucket",
            "lifecycle",
            "rule",
            "import",
            &destination,
            lifecycle
                .path()
                .to_str()
                .context("cache lifecycle path must be UTF-8")?,
        ],
        cancel,
    )?;
    rc(&["admin", "user", "add", "ci", &key, &password], cancel)?;
    let mut policy_file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(&mut policy_file, &policy(scope, &bucket))?;
    rc(
        &[
            "admin",
            "policy",
            "create",
            "ci",
            &bucket,
            policy_file
                .path()
                .to_str()
                .context("cache policy path must be UTF-8")?,
        ],
        cancel,
    )?;
    rc(
        &["admin", "policy", "attach", "ci", &bucket, "--user", &key],
        cancel,
    )?;
    write_environment(&directory, &bucket, endpoint, &key, &password)?;
    let status = child::run(
        Command::new("chown")
            .args(["-R", &uid.to_string()])
            .arg(&directory),
        Some(cancel),
    )?;
    ensure!(status.success(), "cache client ownership failed: {status}");
    info!(%scope, "compiler cache scope initialized");
    Ok(())
}

/// How long each snapshot layer in a scope's bucket lives.
///
/// The compiler cache has no age rule. An age rule measures an entry's write,
/// and a hit does not renew it, so it expired first the entries every build
/// reads, the ones written longest ago. The evictor keeps that layer under the
/// scope's quota by last use instead. The snapshot layers are keyed by content
/// (a target fingerprint, a `Cargo.lock`), so an object still named by a lock
/// file is still the right answer weeks later, and expiring it daily would
/// mean paying the full fetch every morning to rebuild the same bytes.
/// S3 lifecycle applies the earliest matching expiry, so these prefixes must
/// not overlap.
fn retention() -> serde_json::Value {
    json!({
        "Rules": [
            {
                "ID": "target-snapshots", "Status": "Enabled",
                "Filter": {"Prefix": "target-snapshots/"},
                "Expiration": {"Days": 7}
            },
            {
                "ID": "source-snapshots", "Status": "Enabled",
                "Filter": {"Prefix": "source-snapshots/"},
                "Expiration": {"Days": 30}
            }
        ]
    })
}

fn policy(scope: &str, bucket: &str) -> serde_json::Value {
    let mut statements = vec![
        json!({
            "Effect": "Allow",
            "Action": ["s3:ListBucket", "s3:GetBucketLocation"],
            "Resource": [format!("arn:aws:s3:::{bucket}")]
        }),
        json!({
            "Effect": "Allow",
            "Action": ["s3:GetObject", "s3:PutObject"],
            "Resource": [format!("arn:aws:s3:::{bucket}/*")]
        }),
    ];
    if scope != "trusted" {
        let trusted = "kithara-trusted";
        statements.extend([
            json!({
                "Effect": "Allow",
                "Action": ["s3:ListBucket"],
                "Resource": [format!("arn:aws:s3:::{trusted}")],
                "Condition": {"StringLike": {"s3:prefix": ["target-snapshots/*", "source-snapshots/*"]}}
            }),
            json!({
                "Effect": "Allow",
                "Action": ["s3:GetObject"],
                "Resource": [
                    format!("arn:aws:s3:::{trusted}/target-snapshots/*"),
                    format!("arn:aws:s3:::{trusted}/source-snapshots/*")
                ]
            }),
        ]);
    }
    json!({"Version": "2012-10-17", "Statement": statements})
}

fn write_environment(
    directory: &Path,
    bucket: &str,
    endpoint: &str,
    key: &str,
    password: &str,
) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    for (name, value) in provisioned_environment(bucket, endpoint, key, password)? {
        writeln!(file, "{name}={value}")?;
    }
    file.persist(directory.join("cache.env"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop signal during the setup ends it, so the store is asked to stop
    /// while Docker still waits for it.
    #[cfg(unix)]
    #[test]
    fn the_setup_stops_at_a_stop_signal() {
        let _signals = crate::testing::signals();
        let cancel = Cancel::install().unwrap();

        signal_hook::low_level::raise(signal_hook::consts::signal::SIGTERM).unwrap();
        let error = rc(&["alias", "list"], &cancel).expect_err("the setup went on");

        assert!(format!("{error:#}").contains("cancelled"), "{error:#}");
    }

    /// The evictor removes only what sits under the compiler-cache prefix, so
    /// a runner whose environment drops the prefix writes to the bucket root,
    /// where nothing is evicted, until the quota refuses every write.
    #[test]
    fn a_provisioned_environment_reaches_the_client_with_its_key_prefix() {
        let directory = tempfile::tempdir().unwrap();
        write_environment(directory.path(), "bucket", "http://cache", "key", "secret").unwrap();

        let environment = super::super::client_environment(&directory.path().join("cache.env"))
            .expect("the client reads what provisioning writes");

        assert_eq!(
            environment.get("SCCACHE_S3_KEY_PREFIX").map(String::as_str),
            Some(consts::SCCACHE_PREFIX)
        );
    }

    /// Scopes were sized apart on the live host and a shared quota would flatten
    /// them on the next initialize, so a scope names its own and only a scope
    /// that says nothing takes the shared one.
    #[test]
    fn a_scope_keeps_the_quota_it_names() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            env::set_var("CACHE_BUCKET_QUOTA_REVIEW", "800GiB");
        }

        assert_eq!(scope_quota("review", "50GiB").unwrap(), 800 << 30);
        assert_eq!(scope_quota("trusted", "50GiB").unwrap(), 50 << 30);
    }

    /// The store is handed the bytes the evictor keeps the bucket under, so a
    /// size the two could read apart is refused. The store reads zero as no
    /// quota, and the evictor would read it as room for nothing.
    #[test]
    fn a_quota_is_a_whole_number_of_binary_units() {
        assert_eq!(size("400GiB").unwrap(), 400 << 30);
        assert_eq!(size("64MiB").unwrap(), 64 << 20);
        for value in [
            "400GB",
            "400G",
            "400",
            "GiB",
            "+4GiB",
            "4 GiB",
            "0GiB",
            "16777216TiB",
        ] {
            assert!(size(value).is_err(), "{value}");
        }
    }

    #[test]
    fn cache_scope_cannot_escape_its_bucket() {
        for scope in [
            "",
            "../trusted",
            "review/trusted",
            "UPPER",
            "review-",
            "a
b",
        ] {
            assert!(scope_bucket(scope).is_err(), "{scope:?}");
        }
        assert_eq!(scope_bucket("review-1").unwrap(), "kithara-review-1");
    }

    #[test]
    fn untrusted_scopes_can_only_read_trusted_target_snapshots() {
        let review = policy("review", "kithara-review");
        let statements = review["Statement"].as_array().unwrap();
        let trusted = statements
            .iter()
            .map(serde_json::Value::to_string)
            .find(|statement| {
                statement.contains("kithara-trusted/target-snapshots/*")
                    && statement.contains("s3:GetObject")
            })
            .unwrap();
        assert!(trusted.contains("target-snapshots/*"));
        assert!(trusted.contains("s3:GetObject"));
        assert!(!trusted.contains("s3:PutObject"));

        let trusted = policy("trusted", "kithara-trusted").to_string();
        assert!(!trusted.contains("target-snapshots/*"));
    }

    /// The source layer is published by the default branch and read by every
    /// branch. Without this grant a review job asks the trusted bucket for the
    /// layer, is refused, and fetches every dependency from the internet.
    #[test]
    fn untrusted_scopes_read_but_never_write_the_trusted_source_layer() {
        let review = policy("review", "kithara-review").to_string();
        assert!(review.contains("kithara-trusted/source-snapshots/*"));
        assert!(!review.contains(r#"["s3:PutObject"],"Resource":["arn:aws:s3:::kithara-trusted"#));

        let trusted = policy("trusted", "kithara-trusted").to_string();
        assert!(!trusted.contains("source-snapshots/*"));
    }

    #[test]
    fn credential_initialization_preserves_existing_values() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("password");
        let original = secret(&path).unwrap();
        assert_eq!(secret(&path).unwrap(), original);
        assert_eq!(original.len(), 64);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    /// S3 lifecycle applies the earliest matching expiry, so an unfiltered rule would
    /// silently govern the snapshot prefixes too - which is what expired a
    /// content-keyed source layer after a day and would have made a
    /// multi-gigabyte object a daily republish.
    #[test]
    fn each_layer_carries_its_own_retention_and_no_rule_is_unfiltered() {
        let rules = retention();
        let rules = rules["Rules"].as_array().expect("rules");
        assert!(!rules.is_empty());

        for rule in rules {
            let prefix = rule["Filter"]["Prefix"].as_str().expect("prefix");
            assert!(!prefix.is_empty(), "an unfiltered rule governs every layer");
            assert!(prefix.ends_with('/'), "{prefix} must name a whole prefix");
            assert!(rule["Expiration"]["Days"].as_u64().expect("days") > 0);
        }
    }

    /// An age rule measures an entry's write, so it expired first the entries
    /// every build reads. The compiler cache answers to the evictor alone.
    #[test]
    fn no_age_rule_reaches_the_compiler_cache() {
        let compiler = format!("{}/", consts::SCCACHE_PREFIX);
        let rules = retention();

        for rule in rules["Rules"].as_array().expect("rules") {
            let prefix = rule["Filter"]["Prefix"].as_str().expect("prefix");
            assert!(
                !compiler.starts_with(prefix) && !prefix.starts_with(&compiler),
                "{prefix} expires compiler-cache entries by age"
            );
        }
    }

    /// The evictor keeps its records beside the scopes. A scope that could
    /// name that bucket would hand the records to a client key and put them
    /// under a quota and eviction.
    #[test]
    fn no_scope_names_the_recency_bucket() {
        let bucket = scope_bucket("a").expect("a valid scope");
        let prefix = bucket
            .strip_suffix('a')
            .expect("a scope ends its bucket name");

        assert!(
            consts::RECENCY_BUCKET
                .strip_prefix(prefix)
                .is_none_or(|scope| scope_bucket(scope).is_err()),
            "{} is a scope's bucket",
            consts::RECENCY_BUCKET
        );
    }
}
