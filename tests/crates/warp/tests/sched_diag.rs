//! DIAG ONLY, never merged: per-block scheduler evidence for zvuk/kithara#606.
//!
//! Every paced block samples each thread of the test process (run time, run
//! queue wait, time slices, state) and the cgroup's CPU accounting, so a
//! starved capture shows whether the deck's worker ran, waited for a CPU, or
//! slept while the ring ran dry.

use std::{fs, path::Path};

use kithara::platform::time::{Duration, WallInstant};
use serde::Serialize;

#[derive(Serialize)]
struct ThreadSample {
    tid: u32,
    comm: String,
    state: String,
    run_ns: u64,
    wait_ns: u64,
    slices: u64,
}

#[derive(Serialize)]
struct BlockSample {
    block: usize,
    at_us: u64,
    render_us: u64,
    underruns: u64,
    cgroup: Vec<(String, u64)>,
    psi_cpu: Option<String>,
    threads: Vec<ThreadSample>,
}

#[derive(Serialize)]
struct Dump<'a> {
    label: &'a str,
    test: Option<String>,
    parallelism: Option<usize>,
    cpu_max: Option<String>,
    loadavg: Option<String>,
    blocks: &'a [BlockSample],
}

pub(crate) struct SchedLog {
    origin: WallInstant,
    blocks: Vec<BlockSample>,
}

impl SchedLog {
    pub(crate) fn new() -> Self {
        Self {
            origin: WallInstant::now(),
            blocks: Vec::new(),
        }
    }

    pub(crate) fn block(&mut self, block: usize, render: Duration, underruns: u64) {
        let at_us = micros(self.origin.elapsed());
        self.blocks.push(BlockSample {
            block,
            at_us,
            render_us: micros(render),
            underruns,
            cgroup: cgroup_cpu_stat(),
            psi_cpu: read_trimmed("/proc/pressure/cpu"),
            threads: threads(),
        });
    }

    /// Write the log beside the hang dumps the lane uploads, when the capture
    /// starved.
    pub(crate) fn finish(&self, label: &str, starved: bool) {
        if !starved {
            return;
        }
        let Some(dir) = std::env::var_os("KITHARA_HANG_DUMP_DIR") else {
            return;
        };
        let dump = Dump {
            label,
            test: std::env::var("NEXTEST_TEST_NAME").ok(),
            parallelism: std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZeroUsize::get),
            cpu_max: read_trimmed("/sys/fs/cgroup/cpu.max"),
            loadavg: read_trimmed("/proc/loadavg"),
            blocks: &self.blocks,
        };
        let Ok(bytes) = serde_json::to_vec(&dump) else {
            return;
        };
        let _ = fs::create_dir_all(&dir);
        let name = format!(
            "warp-sched-{label}-{}-{}.json",
            std::process::id(),
            micros(self.origin.elapsed())
        );
        let _ = fs::write(Path::new(&dir).join(name), bytes);
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn cgroup_cpu_stat() -> Vec<(String, u64)> {
    read_trimmed("/sys/fs/cgroup/cpu.stat")
        .map(|text| {
            text.lines()
                .filter_map(|line| {
                    let (key, value) = line.split_once(' ')?;
                    Some((key.to_owned(), value.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn threads() -> Vec<ThreadSample> {
    let Ok(entries) = fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let tid = entry.file_name().to_str()?.parse().ok()?;
            let dir = entry.path();
            let comm = fs::read_to_string(dir.join("comm")).ok()?.trim().to_owned();
            let stat = fs::read_to_string(dir.join("stat")).ok()?;
            let state = stat
                .rsplit_once(')')?
                .1
                .split_whitespace()
                .next()?
                .to_owned();
            let schedstat = fs::read_to_string(dir.join("schedstat")).ok()?;
            let mut fields = schedstat
                .split_whitespace()
                .map(|field| field.parse::<u64>().ok());
            Some(ThreadSample {
                tid,
                comm,
                state,
                run_ns: fields.next()??,
                wait_ns: fields.next()??,
                slices: fields.next()??,
            })
        })
        .collect()
}
