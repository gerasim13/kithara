use std::{
    collections::HashMap,
    convert::Infallible,
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail, ensure};
use tiny_http::Server;
use tracing::{info, warn};

use super::{
    super::{provision, required},
    audit::{self, Delivery, Event},
    entry::Entry,
    plan::{self, Listing, Stored, Victim},
    record,
    store::Store,
};
use crate::{ci::host::mac::read_secret, consts};

/// Keeps each scope's compiler cache under the quota the host's environment
/// names for it by evicting the entries used longest ago, as the store's audit
/// log reports their use. The store's buckets carry no quota of their own, so
/// this is the only bound on them.
pub(in crate::ci::cache) fn run() -> Result<Infallible> {
    let shared = required("CACHE_BUCKET_QUOTA")?;
    let buckets = required("CACHE_SCOPES")?
        .split_whitespace()
        .map(|scope| {
            Ok((
                provision::scope_bucket(scope)?,
                provision::scope_quota(scope, &shared)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let server = Server::http(consts::EVICT_LISTEN)
        .map_err(|error| anyhow!("listen for the store's audit log: {error}"))?;
    let (sender, deliveries) = mpsc::sync_channel(Owner::CHANNEL);
    let managed = buckets
        .iter()
        .map(|(bucket, _)| bucket.clone())
        .collect::<Vec<_>>();
    thread::spawn(move || audit::receive(&server, &managed, &sender));
    let config = Path::new("/config");
    let store = Store::connect(
        Path::new("rc"),
        &read_secret(&config.join("admin-user"))?,
        &read_secret(&config.join("admin-password"))?,
    )?;
    Owner::start(store, buckets, deliveries)?.serve()
}

struct Scope {
    bucket: String,
    /// The second each entry was last read, as far as the log has said.
    reads: HashMap<Entry, u64>,
    quota: u64,
    next_pass: Instant,
}

struct Owner {
    store: Store,
    scopes: Vec<Scope>,
    deliveries: Receiver<Delivery>,
    /// Events the receiver dropped since the last pass reported them.
    dropped: u64,
}

impl Owner {
    /// How often each bucket is listed and brought back under its budget.
    const PASS_INTERVAL: Duration = Duration::from_secs(30 * 60);

    /// How long the owner waits on the audit log before it looks at its
    /// clocks.
    const TICK: Duration = Duration::from_secs(1);

    /// Entries removed per request. The log is read between requests, so an
    /// entry a build reads while a pass runs is spared.
    const CHUNK: usize = 500;

    /// Deliveries the receiver holds for the owner while a listing runs.
    const CHANNEL: usize = 65_536;

    /// Reads each scope's record back. A record that does not read back
    /// whole is set aside, which only ages its entries; a store that cannot
    /// be read stops the start, and so does a scope listed twice, since the
    /// log credits each bucket to its first scope alone.
    fn start(
        store: Store,
        buckets: Vec<(String, u64)>,
        deliveries: Receiver<Delivery>,
    ) -> Result<Self> {
        for (index, (bucket, _)) in buckets.iter().enumerate() {
            ensure!(
                !buckets[..index].iter().any(|(other, _)| other == bucket),
                "the scopes name {bucket} twice"
            );
        }
        let now = Instant::now();
        let mut scopes = Vec::with_capacity(buckets.len());
        for (bucket, quota) in buckets {
            let reads = store
                .get(consts::RECENCY_BUCKET, &bucket)?
                .map_or_else(HashMap::new, |bytes| {
                    record::decode(&bytes).unwrap_or_else(|error| {
                        warn!(%bucket, error = format!("{error:#}"), "setting aside a record of reads that does not read back");
                        HashMap::new()
                    })
                });
            scopes.push(Scope {
                bucket,
                reads,
                quota,
                next_pass: now,
            });
        }
        Ok(Self {
            store,
            scopes,
            deliveries,
            dropped: 0,
        })
    }

    fn serve(mut self) -> Result<Infallible> {
        loop {
            match self.deliveries.recv_timeout(Self::TICK) {
                Ok(delivery) => self.absorb(delivery),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => bail!("the audit log receiver stopped"),
            }
            for index in 0..self.scopes.len() {
                self.tick(index);
            }
        }
    }

    fn tick(&mut self, index: usize) {
        let now = Instant::now();
        let scope = &mut self.scopes[index];
        if scope.next_pass <= now {
            scope.next_pass = now + Self::PASS_INTERVAL;
            if let Err(error) = self.pass(index) {
                warn!(bucket = %self.scopes[index].bucket, error = format!("{error:#}"), "eviction pass failed");
            }
        }
    }

    fn drain(&mut self) {
        while let Ok(delivery) = self.deliveries.try_recv() {
            self.absorb(delivery);
        }
    }

    fn absorb(&mut self, delivery: Delivery) {
        self.dropped = self.dropped.saturating_add(delivery.dropped);
        for Event { scope, entry, at_s } in delivery.events {
            self.scopes[scope]
                .reads
                .entry(entry)
                .and_modify(|read| *read = (*read).max(at_s))
                .or_insert(at_s);
        }
    }

    /// Lists the bucket, evicts what the budget does not hold, and stores the
    /// record of reads. A listing that fails evicts nothing: a partial one
    /// would undercount the bucket and forget the reads of what it missed.
    fn pass(&mut self, index: usize) -> Result<()> {
        self.drain();
        let bucket = self.scopes[index].bucket.clone();
        let quota = self.scopes[index].quota;
        let started = Instant::now();
        let listing = self.listing(&bucket)?;
        let listing_ms = started.elapsed().as_millis();
        let scope = &mut self.scopes[index];
        plan::prune(&mut scope.reads, &listing);
        let chosen = plan::victims(&listing, &scope.reads, quota);
        let evicted = self.evict(index, &chosen)?;
        let scope = &self.scopes[index];
        self.store.put(
            consts::RECENCY_BUCKET,
            &bucket,
            &record::encode(&scope.reads),
        )?;
        info!(
            %bucket,
            entries = listing.entries.len(),
            used = listing.used(),
            quota,
            listing_ms,
            evicted = evicted.len(),
            evicted_bytes = evicted.iter().map(|victim| victim.size).fold(0, u64::saturating_add),
            evicted_unread = evicted.iter().filter(|victim| !victim.read).count(),
            horizon_s = evicted.last().map(|victim| victim.used_s),
            reads = scope.reads.len(),
            dropped_events = self.dropped,
            "eviction pass"
        );
        self.dropped = 0;
        Ok(())
    }

    /// Lists the whole bucket. The compiler cache is listed a shard at a
    /// time, so no one listing holds it all, and the log is read between
    /// listings, so a long pass does not leave the receiver dropping events.
    fn listing(&mut self, bucket: &str) -> Result<Listing> {
        let compiler = format!("{}/", consts::SCCACHE_PREFIX);
        let mut listing = Listing::default();
        let mut pending = vec![(String::new(), false)];
        while let Some((prefix, recursive)) = pending.pop() {
            let page = self.store.list(bucket, &prefix, recursive)?;
            self.drain();
            for object in page.objects {
                listing.insert(
                    &object.key,
                    Stored {
                        size: object.size,
                        written_s: object.written_s,
                    },
                );
            }
            pending.extend(page.prefixes.into_iter().map(|child| {
                let recursive = child != compiler;
                (child, recursive)
            }));
        }
        Ok(listing)
    }

    /// Removes the chosen entries a chunk at a time, sparing any the log has
    /// seen used since they were chosen.
    fn evict(&mut self, index: usize, chosen: &[Victim]) -> Result<Vec<Victim>> {
        let mut evicted = Vec::new();
        for chunk in chosen.chunks(Self::CHUNK) {
            self.drain();
            let scope = &self.scopes[index];
            let unused = plan::still_unused(chunk, &scope.reads);
            if unused.is_empty() {
                continue;
            }
            let objects = unused
                .iter()
                .map(|victim| victim.entry.object())
                .collect::<Vec<_>>();
            self.store.remove(&scope.bucket, &objects)?;
            evicted.extend(unused);
        }
        Ok(evicted)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;
    use crate::testing::install_script;

    fn hash(first: char, fill: char) -> String {
        std::iter::once(first)
            .chain(std::iter::repeat_n(fill, 63))
            .collect()
    }

    fn object(first: char, fill: char) -> String {
        let hash = hash(first, fill);
        format!("sccache/{first}/{fill}/{fill}/{hash}")
    }

    fn entry(first: char, fill: char) -> Entry {
        Entry::parse(&object(first, fill)).expect("a compiler cache object names an entry")
    }

    /// A bucket at the quota [`owner`] gives it: the startup probe, three
    /// entries of 300 bytes written at seconds 100, 200 and 300, and an
    /// 87-byte snapshot. `child_b` answers the listing under `sccache/b/`.
    fn bucket(child_b: &str) -> String {
        let item = |key: &str, size: u64, at: &str| {
            format!(
                r#"{{"key":"{key}","size_bytes":{size},"last_modified":"1970-01-01T00:{at}Z","is_dir":false}}"#
            )
        };
        format!(
            r#""--json object list ci/kithara-review/") printf '%s' '{{"items":[{{"key":"sccache/","is_dir":true}},{{"key":"target-snapshots/","is_dir":true}}],"truncated":false}}' ;;
"--json object list ci/kithara-review/sccache/") printf '%s' '{{"items":[{{"key":"sccache/a/","is_dir":true}},{{"key":"sccache/b/","is_dir":true}},{probe}],"truncated":false}}' ;;
"--json object list --recursive ci/kithara-review/sccache/a/") printf '%s' '{{"items":[{a1},{a2}],"truncated":false}}' ;;
"--json object list --recursive ci/kithara-review/sccache/b/") {child_b} ;;
"--json object list --recursive ci/kithara-review/target-snapshots/") printf '%s' '{{"items":[{snapshot}],"truncated":false}}' ;;
"object remove --force "*) ;;"#,
            probe = item("sccache/.sccache_check", 13, "00:00"),
            a1 = item(&object('a', '1'), 300, "01:40.5"),
            a2 = item(&object('a', '2'), 300, "03:20"),
            snapshot = item("target-snapshots/f/c.tar", 87, "00:00"),
        )
    }

    fn b1_listing() -> String {
        format!(
            r#"printf '%s' '{{"items":[{{"key":"{}","size_bytes":300,"last_modified":"1970-01-01T00:05:00Z","is_dir":false}}],"truncated":false}}'"#,
            object('b', '1')
        )
    }

    /// Speaks `rc` for one review bucket: logs every call and keeps the
    /// record it is given.
    fn rc(directory: &Path, cases: &str) -> PathBuf {
        let program = directory.join("rc");
        let log = directory.join("log");
        install_script(
            &program,
            &format!(
                r#"#!/bin/sh
echo "$*" >> '{log}'
case "$*" in
"alias set -- ci {store} user password") ;;
"object show ci/ci-cache-recency/kithara-review") echo "Not found" >&2; exit 5 ;;
"pipe ci/ci-cache-recency/kithara-review") cat > '{record}' ;;
{cases}
*) echo "unexpected: $*" >&2; exit 2 ;;
esac
"#,
                log = log.display(),
                store = consts::CACHE_STORE_URL,
                record = directory.join("record").display(),
            ),
        );
        program
    }

    fn calls(directory: &Path) -> Vec<String> {
        fs::read_to_string(directory.join("log"))
            .expect("the stand-in store logs its calls")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn owner(program: &Path) -> (Owner, mpsc::SyncSender<Delivery>) {
        let (sender, deliveries) = mpsc::sync_channel(16);
        let store =
            Store::connect(program, "user", "password").expect("the stand-in store connects");
        let owner = Owner::start(store, vec![("kithara-review".to_owned(), 1000)], deliveries)
            .expect("the owner starts on a readable store");
        (owner, sender)
    }

    fn deliver(sender: &mpsc::SyncSender<Delivery>, events: Vec<Event>) {
        sender
            .send(Delivery { events, dropped: 0 })
            .expect("the owner holds the receiving end");
    }

    /// The age rule this replaces expired the entry every build reads. Here
    /// the oldest write survives because it was read last, and the two
    /// entries nothing read since go in one request.
    #[test]
    fn a_pass_evicts_what_was_used_longest_ago_and_keeps_the_record() {
        let directory = tempfile::tempdir().unwrap();
        let program = rc(directory.path(), &bucket(&b1_listing()));
        let (mut owner, sender) = owner(&program);
        deliver(
            &sender,
            vec![Event {
                scope: 0,
                entry: entry('a', '1'),
                at_s: 400,
            }],
        );

        owner.pass(0).unwrap();

        let removals = calls(directory.path())
            .into_iter()
            .filter(|call| call.starts_with("object remove"))
            .collect::<Vec<_>>();
        assert_eq!(
            removals,
            [format!(
                "object remove --force ci/kithara-review/{} ci/kithara-review/{}",
                object('a', '2'),
                object('b', '1')
            )]
        );
        let record = record::decode(&fs::read(directory.path().join("record")).unwrap()).unwrap();
        assert_eq!(record, HashMap::from([(entry('a', '1'), 400)]));
    }

    #[test]
    fn a_pass_whose_listing_fails_evicts_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let program = rc(
            directory.path(),
            &bucket(r#"echo "Access denied" >&2; exit 4"#),
        );
        let (mut owner, _sender) = owner(&program);

        assert!(owner.pass(0).is_err());

        let calls = calls(directory.path());
        assert!(
            !calls
                .iter()
                .any(|call| call.starts_with("object remove") || call.starts_with("pipe")),
            "{calls:?}"
        );
    }

    /// Every delivery names its bucket's first scope, so a scope listed twice
    /// would evict by writes alone and overwrite the first one's record.
    #[test]
    fn a_scope_listed_twice_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let program = rc(directory.path(), "");
        let store = Store::connect(&program, "user", "password").unwrap();

        let started = Owner::start(
            store,
            vec![
                ("kithara-review".to_owned(), 1000),
                ("kithara-review".to_owned(), 1000),
            ],
            mpsc::sync_channel(1).1,
        );

        assert!(started.is_err());
    }

    #[test]
    fn only_a_record_that_does_not_read_back_is_set_aside() {
        let directory = tempfile::tempdir().unwrap();
        let start = |show: &str| {
            let program = directory.path().join("show");
            install_script(
                &program,
                &format!(
                    r#"#!/bin/sh
case "$1" in
alias) ;;
object) {show} ;;
esac
"#
                ),
            );
            let store = Store::connect(&program, "user", "password").unwrap();
            Owner::start(
                store,
                vec![("kithara-review".to_owned(), 1000)],
                mpsc::sync_channel(1).1,
            )
        };

        let garbled = start("printf 'garbled'").unwrap();
        assert!(garbled.scopes[0].reads.is_empty());
        assert!(start(r#"echo "Access denied" >&2; exit 4"#).is_err());
    }
}
