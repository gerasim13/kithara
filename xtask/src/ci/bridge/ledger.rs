use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use fs4::FileExt;
use serde::{Deserialize, Serialize};

use super::model::ImportState;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LedgerEntry {
    pub(super) state: ImportState,
    pub(super) pipeline_id: Option<u64>,
    pub(super) check_run_id: Option<u64>,
    pub(super) detail: Option<String>,
    updated_at: u64,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LedgerData {
    imports: BTreeMap<String, LedgerEntry>,
}

pub(super) struct Ledger {
    path: PathBuf,
    lock_path: PathBuf,
}

impl Ledger {
    pub(super) fn new(state_dir: &Path) -> Result<Self> {
        fs::create_dir_all(state_dir)
            .with_context(|| format!("creating bridge state {}", state_dir.display()))?;
        Ok(Self {
            path: state_dir.join("ledger.json"),
            lock_path: state_dir.join("ledger.lock"),
        })
    }

    pub(super) fn get(
        &self,
        github_sha: &str,
        gitlab_base_sha: &str,
    ) -> Result<Option<LedgerEntry>> {
        self.with_locked_data(|data| {
            Ok(data
                .imports
                .get(&ledger_key(github_sha, gitlab_base_sha))
                .cloned())
        })
    }

    pub(super) fn put(
        &self,
        github_sha: &str,
        gitlab_base_sha: &str,
        state: ImportState,
        pipeline_id: Option<u64>,
        check_run_id: Option<u64>,
        detail: Option<String>,
    ) -> Result<()> {
        self.with_locked_data(|data| {
            let key = ledger_key(github_sha, gitlab_base_sha);
            let previous = data.imports.get(&key);
            data.imports.insert(
                key,
                LedgerEntry {
                    state,
                    pipeline_id: pipeline_id
                        .or_else(|| previous.and_then(|entry| entry.pipeline_id)),
                    check_run_id: check_run_id
                        .or_else(|| previous.and_then(|entry| entry.check_run_id)),
                    detail,
                    updated_at: unix_time()?,
                },
            );
            self.write(data)
        })
    }

    fn with_locked_data<T>(
        &self,
        operation: impl FnOnce(&mut LedgerData) -> Result<T>,
    ) -> Result<T> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .with_context(|| format!("opening ledger lock {}", self.lock_path.display()))?;
        FileExt::lock(&lock)
            .with_context(|| format!("locking ledger {}", self.lock_path.display()))?;
        let mut data = self.read()?;
        let result = operation(&mut data);
        FileExt::unlock(&lock)
            .with_context(|| format!("unlocking ledger {}", self.lock_path.display()))?;
        result
    }

    fn read(&self) -> Result<LedgerData> {
        if !self.path.exists() {
            return Ok(LedgerData::default());
        }
        let text = fs::read_to_string(&self.path)
            .with_context(|| format!("reading bridge ledger {}", self.path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing bridge ledger {}", self.path.display()))
    }

    fn write(&self, data: &LedgerData) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(data).context("serializing bridge ledger")?;
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_nanos();
        let temporary =
            self.path
                .with_extension(format!("json.{}.{}.tmp", std::process::id(), suffix));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .with_context(|| format!("creating temporary ledger {}", temporary.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("writing temporary ledger {}", temporary.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("terminating temporary ledger {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing temporary ledger {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &self.path).with_context(|| {
            format!(
                "publishing bridge ledger {} as {}",
                temporary.display(),
                self.path.display()
            )
        })
    }
}

fn ledger_key(github_sha: &str, gitlab_base_sha: &str) -> String {
    format!("{github_sha}:{gitlab_base_sha}")
}

fn unix_time() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_is_idempotent_and_preserves_ids() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(directory.path()).unwrap();
        ledger
            .put(
                "github",
                "gitlab",
                ImportState::Testing,
                Some(42),
                Some(84),
                None,
            )
            .unwrap();
        ledger
            .put(
                "github",
                "gitlab",
                ImportState::Promoted,
                None,
                None,
                Some("done".into()),
            )
            .unwrap();

        let entry = ledger.get("github", "gitlab").unwrap().unwrap();
        assert_eq!(entry.state, ImportState::Promoted);
        assert_eq!(entry.pipeline_id, Some(42));
        assert_eq!(entry.check_run_id, Some(84));
        assert_eq!(entry.detail.as_deref(), Some("done"));
    }
}
