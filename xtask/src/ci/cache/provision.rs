use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};
use serde_json::json;
use tracing::info;

use super::required;
use crate::ci::host::{read_secret, write_secure};

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

fn mc(arguments: &[&str]) -> Result<()> {
    let status = Command::new("mc")
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("start cache administration client")?;
    // Arguments and client diagnostics can include credentials.
    ensure!(
        status.success(),
        "cache administration operation failed: {status}"
    );
    Ok(())
}

fn scope_bucket(scope: &str) -> Result<String> {
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

pub(super) fn initialize() -> Result<()> {
    let scopes = required("CACHE_SCOPES")?;
    let quota = required("CACHE_BUCKET_QUOTA")?;
    let endpoint = required("CACHE_CLIENT_ENDPOINT")?;
    let uid = required("CACHE_CLIENT_UID")?.parse::<u32>()?;
    let url = reqwest::Url::parse(&endpoint)?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && !endpoint.chars().any(char::is_whitespace),
        "cache endpoint must be an HTTP URL without whitespace"
    );
    for scope in scopes.split_whitespace() {
        scope_bucket(scope)?;
    }
    let root = Path::new("/config");
    mc(&[
        "alias",
        "set",
        "--",
        "ci",
        "http://cache:9000",
        &read_secret(&root.join("admin-user"))?,
        &read_secret(&root.join("admin-password"))?,
    ])?;
    for scope in scopes.split_whitespace() {
        initialize_scope(scope, &quota, &endpoint, uid)?;
    }
    Ok(())
}

fn initialize_scope(scope: &str, quota: &str, endpoint: &str, uid: u32) -> Result<()> {
    let bucket = scope_bucket(scope)?;
    let destination = format!("ci/{bucket}");
    let directory = Path::new("/clients").join(scope);
    fs::create_dir_all(&directory)?;
    let key = secret(&directory.join("access-key"))?;
    let password = secret(&directory.join("secret-key"))?;
    mc(&["mb", "--ignore-existing", &destination])?;
    mc(&["quota", "set", &destination, "--size", quota])?;
    let mut lifecycle = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(
        &mut lifecycle,
        &json!({
            "Rules": [{
                "ID": "cache-retention", "Status": "Enabled",
                "Filter": {"Prefix": ""}, "Expiration": {"Days": 1}
            }]
        }),
    )?;
    let status = Command::new("mc")
        .args(["ilm", "rule", "import", &destination])
        .stdin(File::open(lifecycle.path())?)
        .stdout(Stdio::null())
        .status()?;
    ensure!(status.success(), "cache lifecycle import failed: {status}");
    mc(&["admin", "user", "add", "ci", &key, &password])?;
    let mut policy_file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(&mut policy_file, &policy(scope, &bucket))?;
    mc(&[
        "admin",
        "policy",
        "create",
        "ci",
        &bucket,
        policy_file
            .path()
            .to_str()
            .context("cache policy path must be UTF-8")?,
    ])?;
    mc(&["admin", "policy", "attach", "ci", &bucket, "--user", &key])?;
    write_environment(&directory, &bucket, endpoint, &key, &password)?;
    let status = Command::new("chown")
        .args(["-R", &uid.to_string()])
        .arg(&directory)
        .status()?;
    ensure!(status.success(), "cache client ownership failed: {status}");
    info!(%scope, "compiler cache scope initialized");
    Ok(())
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
                "Condition": {"StringLike": {"s3:prefix": ["target-snapshots/*"]}}
            }),
            json!({
                "Effect": "Allow",
                "Action": ["s3:GetObject"],
                "Resource": [format!("arn:aws:s3:::{trusted}/target-snapshots/*")]
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
    for (name, value) in [
        ("SCCACHE_BUCKET", bucket),
        ("SCCACHE_ENDPOINT", endpoint),
        ("SCCACHE_REGION", "us-east-1"),
        (
            "SCCACHE_S3_USE_SSL",
            if endpoint.starts_with("https://") {
                "true"
            } else {
                "false"
            },
        ),
        ("AWS_ACCESS_KEY_ID", key),
        ("AWS_SECRET_ACCESS_KEY", password),
        ("AWS_EC2_METADATA_DISABLED", "true"),
    ] {
        writeln!(file, "{name}={value}")?;
    }
    file.persist(directory.join("cache.env"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn credential_initialization_preserves_existing_values() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("password");
        let original = secret(&path).unwrap();
        assert_eq!(secret(&path).unwrap(), original);
        assert_eq!(original.len(), 64);
    }
}
