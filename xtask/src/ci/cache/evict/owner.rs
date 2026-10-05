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

/// Keeps each scope's compiler cache under the quota the setup gave its
/// bucket by evicting the entries used longest ago, as the store's audit log
/// reports their use.
///
/// The quota is read from the environment the setup applied, not asked of
/// the store: after a start the store answers no quota question until it has
/// counted the bucket, a minute or more.
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

/// What the evictor knows of whether the audit log is arriving, which a
/// recount waits on: a bucket that is quiet because the log stopped is not
/// quiet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Liveness {
    Unknown,
    /// The marker was asked for at this moment and its echo has not arrived.
    Probed(Instant),
    /// The marker's echo arrived, and no request has since.
    Echoed,
    /// The echo never arrived, or the recount failed; nothing is recounted
    /// until the next pass.
    Silent,
}

struct Scope {
    bucket: String,
    /// The second each entry was last read, as far as the log has said.
    reads: HashMap<Entry, u64>,
    quota: u64,
    /// Bytes deleted since the store last recounted the bucket, or none when
    /// the evictor does not know.
    unreconciled: Option<u64>,
    active_at: Instant,
    liveness: Liveness,
    next_pass: Instant,
}

impl Scope {
    /// A client request: the quiet minute starts over, and an echo that came
    /// before it proves nothing about the minute after.
    fn busy(&mut self) {
        self.active_at = Instant::now();
        self.liveness = Liveness::Unknown;
    }
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

    /// How long a bucket has to go without a client request before the store
    /// is made to recount it. The recount holds the bucket's quota lock while
    /// it walks every object, and a write that waits on that lock longer than
    /// five seconds fails; a lane whose startup probe fails that way runs its
    /// whole compiler cache read-only.
    const QUIET: Duration = Duration::from_secs(60);

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
                unreconciled: None,
                active_at: now,
                liveness: Liveness::Unknown,
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
            if scope.liveness == Liveness::Silent {
                scope.liveness = Liveness::Unknown;
            }
            if let Err(error) = self.pass(index) {
                warn!(bucket = %self.scopes[index].bucket, error = format!("{error:#}"), "eviction pass failed");
            }
        }
        if let Err(error) = self.tend(index) {
            let scope = &mut self.scopes[index];
            scope.liveness = Liveness::Silent;
            warn!(bucket = %scope.bucket, error = format!("{error:#}"), "recount failed; no recount before the next pass");
        }
    }

    fn drain(&mut self) {
        while let Ok(delivery) = self.deliveries.try_recv() {
            self.absorb(delivery);
        }
    }

    fn absorb(&mut self, delivery: Delivery) {
        self.dropped = self.dropped.saturating_add(delivery.dropped);
        for event in delivery.events {
            match event {
                Event::Use { scope, entry, at_s } => {
                    let scope = &mut self.scopes[scope];
                    scope
                        .reads
                        .entry(entry)
                        .and_modify(|read| *read = (*read).max(at_s))
                        .or_insert(at_s);
                    scope.busy();
                }
                Event::Echo { scope } => {
                    let scope = &mut self.scopes[scope];
                    if matches!(scope.liveness, Liveness::Probed(_)) {
                        scope.liveness = Liveness::Echoed;
                    }
                }
                Event::Activity { scope } => self.scopes[scope].busy(),
            }
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
            unreconciled = scope.unreconciled,
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
            let scope = &mut self.scopes[index];
            let unused = plan::still_unused(chunk, &scope.reads);
            if unused.is_empty() {
                continue;
            }
            let objects = unused
                .iter()
                .map(|victim| victim.entry.object())
                .collect::<Vec<_>>();
            if let Err(error) = self.store.remove(&scope.bucket, &objects) {
                // Some of the chunk may be gone, and how much is unknown.
                scope.unreconciled = None;
                return Err(error);
            }
            let bytes = unused
                .iter()
                .map(|victim| victim.size)
                .fold(0, u64::saturating_add);
            scope.unreconciled = scope
                .unreconciled
                .map(|unreconciled| unreconciled.saturating_add(bytes));
            evicted.extend(unused);
        }
        Ok(evicted)
    }

    /// Moves a bucket that wants a recount towards one: a quiet bucket is
    /// probed, and recounted once the probe's echo shows the log is arriving.
    /// It decides on every request the log has handed over, not only on the
    /// one that woke the owner.
    fn tend(&mut self, index: usize) -> Result<()> {
        self.drain();
        let scope = &mut self.scopes[index];
        if !plan::recount_wanted(scope.unreconciled, scope.quota)
            || scope.active_at.elapsed() < Self::QUIET
        {
            return Ok(());
        }
        match scope.liveness {
            Liveness::Unknown => {
                self.store.probe(&scope.bucket, consts::EVICT_MARKER)?;
                scope.liveness = Liveness::Probed(Instant::now());
            }
            Liveness::Probed(at) if at.elapsed() > Self::QUIET => {
                warn!(bucket = %scope.bucket, "the audit log did not echo the probe; no recount before the next pass");
                scope.liveness = Liveness::Silent;
            }
            Liveness::Probed(_) | Liveness::Silent => {}
            Liveness::Echoed => {
                let started = Instant::now();
                // The store recounts on the write after one that shrank an
                // object.
                for body in [&b".."[..], b".", b"."] {
                    self.store.put(&scope.bucket, consts::EVICT_MARKER, body)?;
                }
                scope.unreconciled = Some(0);
                scope.liveness = Liveness::Unknown;
                info!(bucket = %scope.bucket, recount_ms = started.elapsed().as_millis(), "the store recounted the bucket");
            }
        }
        Ok(())
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
        Entry::parse(&object(first, fill)).unwrap()
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

    /// Speaks `rc` for one review bucket: logs every call, keeps the record
    /// it is given, and logs how many bytes each marker write carried.
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
"pipe ci/kithara-review/.evict/recount") wc -c | tr -d ' ' >> '{log}' ;;
"object stat ci/kithara-review/.evict/recount") echo "Not found" >&2; exit 5 ;;
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
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn owner(program: &Path) -> (Owner, mpsc::SyncSender<Delivery>) {
        let (sender, deliveries) = mpsc::sync_channel(16);
        let store = Store::connect(program, "user", "password").unwrap();
        let owner =
            Owner::start(store, vec![("kithara-review".to_owned(), 1000)], deliveries).unwrap();
        (owner, sender)
    }

    fn deliver(sender: &mpsc::SyncSender<Delivery>, events: Vec<Event>) {
        sender.send(Delivery { events, dropped: 0 }).unwrap();
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
            vec![Event::Use {
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

    /// After a start the store refuses every quota question until it has
    /// counted the bucket, a minute or more; a pass that waited on that answer
    /// left a full bucket full until the next pass, half an hour later.
    #[test]
    fn the_first_pass_evicts_before_the_store_has_counted_the_bucket() {
        let directory = tempfile::tempdir().unwrap();
        let uncounted = r#""--json bucket quota info ci/kithara-review") echo 'HTTP 503: authoritative bucket usage is not available yet' >&2; exit 1 ;;"#;
        let program = rc(
            directory.path(),
            &format!("{uncounted}\n{}", bucket(&b1_listing())),
        );
        let (mut owner, _sender) = owner(&program);

        owner.pass(0).unwrap();

        let calls = calls(directory.path());
        assert!(
            calls.iter().any(|call| call.starts_with("object remove")),
            "{calls:?}"
        );
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

    /// The store recounts a bucket only on a write that shrinks the marker
    /// and then on the next write to it, so the marker is written three
    /// times, smaller each time but the last.
    #[test]
    fn a_quiet_bucket_is_probed_and_recounted_once_the_log_echoes() {
        let directory = tempfile::tempdir().unwrap();
        let program = rc(directory.path(), "");
        let (mut owner, sender) = owner(&program);
        owner.scopes[0].active_at = Instant::now().checked_sub(Owner::QUIET).unwrap();

        owner.tend(0).unwrap();
        deliver(&sender, vec![Event::Echo { scope: 0 }]);
        owner.drain();
        owner.tend(0).unwrap();

        let calls = calls(directory.path());
        let marker = calls
            .iter()
            .skip_while(|call| !call.starts_with("object stat"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            marker,
            [
                "object stat ci/kithara-review/.evict/recount",
                "pipe ci/kithara-review/.evict/recount",
                "2",
                "pipe ci/kithara-review/.evict/recount",
                "1",
                "pipe ci/kithara-review/.evict/recount",
                "1",
            ]
        );
        assert_eq!(owner.scopes[0].unreconciled, Some(0));
        assert_eq!(owner.scopes[0].liveness, Liveness::Unknown);
    }

    /// A request that arrives after the probe was made restarts the quiet
    /// minute, and the echo behind it proves nothing about the minute after.
    #[test]
    fn a_request_after_the_probe_holds_the_recount() {
        let directory = tempfile::tempdir().unwrap();
        let program = rc(directory.path(), "");
        let (mut owner, sender) = owner(&program);
        owner.scopes[0].active_at = Instant::now().checked_sub(Owner::QUIET).unwrap();

        owner.tend(0).unwrap();
        deliver(
            &sender,
            vec![Event::Activity { scope: 0 }, Event::Echo { scope: 0 }],
        );
        owner.drain();
        owner.tend(0).unwrap();

        let calls = calls(directory.path());
        assert!(
            !calls.iter().any(|call| call.starts_with("pipe")),
            "{calls:?}"
        );
        assert_eq!(owner.scopes[0].liveness, Liveness::Unknown);
        assert_eq!(owner.scopes[0].unreconciled, None);
    }

    /// The log arrives a request at a time, so the echo can be taken while
    /// the request behind it still waits in the channel.
    #[test]
    fn a_request_queued_behind_the_echo_holds_the_recount() {
        let directory = tempfile::tempdir().unwrap();
        let program = rc(directory.path(), "");
        let (mut owner, sender) = owner(&program);
        owner.scopes[0].active_at = Instant::now().checked_sub(Owner::QUIET).unwrap();

        owner.tend(0).unwrap();
        deliver(&sender, vec![Event::Echo { scope: 0 }]);
        owner.drain();
        deliver(&sender, vec![Event::Activity { scope: 0 }]);
        owner.tend(0).unwrap();

        let calls = calls(directory.path());
        assert!(
            !calls.iter().any(|call| call.starts_with("pipe")),
            "{calls:?}"
        );
        assert_eq!(owner.scopes[0].liveness, Liveness::Unknown);
    }

    /// A store that refuses the probe would refuse it every second; the
    /// next pass tries again, and the log carries one warning in between.
    #[test]
    fn a_recount_that_fails_waits_for_the_next_pass() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("rc");
        install_script(
            &program,
            &format!(
                r#"#!/bin/sh
echo "$*" >> '{log}'
case "$*" in
"alias set -- ci {store} user password") ;;
"object show ci/ci-cache-recency/kithara-review") exit 5 ;;
"object stat ci/kithara-review/.evict/recount") echo "Access denied" >&2; exit 4 ;;
*) echo "unexpected: $*" >&2; exit 2 ;;
esac
"#,
                log = directory.path().join("log").display(),
                store = consts::CACHE_STORE_URL,
            ),
        );
        let (mut owner, _sender) = owner(&program);
        owner.scopes[0].active_at = Instant::now().checked_sub(Owner::QUIET).unwrap();
        owner.scopes[0].next_pass = Instant::now() + Owner::PASS_INTERVAL;

        owner.tick(0);
        owner.tick(0);

        let probes = calls(directory.path())
            .into_iter()
            .filter(|call| call.starts_with("object stat"))
            .count();
        assert_eq!(probes, 1);
        assert_eq!(owner.scopes[0].liveness, Liveness::Silent);
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
