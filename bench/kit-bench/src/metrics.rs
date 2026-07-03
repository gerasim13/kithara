use std::time::Instant;

fn rusage_self() -> libc::rusage {
    // SAFETY: `libc::rusage` is a plain C out-param type, and getrusage fills
    // it for `RUSAGE_SELF`. If the syscall fails, the zeroed value keeps the
    // benchmark error path non-panicking.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `&mut ru` is valid for writes for the duration of the call.
    let _ = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    ru
}

fn tv_secs(tv: libc::timeval) -> f64 {
    tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6
}

pub(crate) struct Baseline {
    pub(crate) t0: Instant,
    ru0_user: f64,
    ru0_sys: f64,
}

impl Baseline {
    pub(crate) fn take() -> Self {
        let ru = rusage_self();
        Self {
            t0: Instant::now(),
            ru0_user: tv_secs(ru.ru_utime),
            ru0_sys: tv_secs(ru.ru_stime),
        }
    }

    pub(crate) fn cpu_delta(&self) -> (f64, f64) {
        let ru = rusage_self();
        (
            tv_secs(ru.ru_utime) - self.ru0_user,
            tv_secs(ru.ru_stime) - self.ru0_sys,
        )
    }

    pub(crate) fn elapsed_ms(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * 1e3
    }
}

pub(crate) struct Report {
    pub(crate) engine: &'static str,
    pub(crate) decoder: String,
    pub(crate) ttfa_ms: f64,
    pub(crate) wall_ms: f64,
    pub(crate) cpu_user_s: f64,
    pub(crate) cpu_sys_s: f64,
    pub(crate) cpu_total_user_s: f64,
    pub(crate) cpu_total_sys_s: f64,
    pub(crate) max_rss_bytes: i64,
    pub(crate) samples: u64,
    pub(crate) pcm_frames: u64,
    pub(crate) samplerate: u32,
    pub(crate) channels: u16,
}

impl Report {
    pub(crate) fn finish(
        baseline: &Baseline,
        decoder: String,
        ttfa_ms: f64,
        samples: u64,
        samplerate: u32,
        channels: u16,
    ) -> Self {
        let (cpu_user_s, cpu_sys_s) = baseline.cpu_delta();
        let ru = rusage_self();
        Self {
            engine: "kithara",
            decoder,
            ttfa_ms,
            wall_ms: baseline.elapsed_ms(),
            cpu_user_s,
            cpu_sys_s,
            cpu_total_user_s: tv_secs(ru.ru_utime),
            cpu_total_sys_s: tv_secs(ru.ru_stime),
            max_rss_bytes: ru.ru_maxrss,
            samples,
            pcm_frames: samples / u64::from(channels.max(1)),
            samplerate,
            channels,
        }
    }

    #[cfg(test)]
    pub(crate) fn duration_secs(&self) -> f64 {
        self.pcm_frames as f64 / f64::from(self.samplerate)
    }

    pub(crate) fn to_json_line(&self) -> String {
        format!(
            concat!(
                "{{\"engine\":\"{}\",\"decoder\":\"{}\",",
                "\"ttfa_ms\":{:.2},\"wall_ms\":{:.2},",
                "\"cpu_user_s\":{:.4},\"cpu_sys_s\":{:.4},",
                "\"cpu_total_user_s\":{:.4},\"cpu_total_sys_s\":{:.4},",
                "\"max_rss_bytes\":{},\"samples\":{},",
                "\"pcm_frames\":{},\"samplerate\":{},\"channels\":{}}}"
            ),
            self.engine,
            self.decoder,
            self.ttfa_ms,
            self.wall_ms,
            self.cpu_user_s,
            self.cpu_sys_s,
            self.cpu_total_user_s,
            self.cpu_total_sys_s,
            self.max_rss_bytes,
            self.samples,
            self.pcm_frames,
            self.samplerate,
            self.channels
        )
    }

    #[cfg(test)]
    pub(crate) fn example(samples: u64, channels: u16, samplerate: u32) -> Self {
        Self {
            engine: "kithara",
            decoder: "symphonia".to_owned(),
            ttfa_ms: 1.0,
            wall_ms: 2.0,
            cpu_user_s: 0.1,
            cpu_sys_s: 0.1,
            cpu_total_user_s: 0.2,
            cpu_total_sys_s: 0.2,
            max_rss_bytes: 1,
            samples,
            pcm_frames: samples / u64::from(channels.max(1)),
            samplerate,
            channels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_frames_is_samples_over_channels() {
        let r = Report::example(88_200, 2, 44_100);
        assert_eq!(r.pcm_frames, 44_100);
        assert!((r.duration_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn json_line_has_contract_keys() {
        let line = Report::example(88_200, 2, 44_100).to_json_line();
        for key in [
            "engine",
            "decoder",
            "ttfa_ms",
            "wall_ms",
            "cpu_user_s",
            "cpu_sys_s",
            "cpu_total_user_s",
            "cpu_total_sys_s",
            "max_rss_bytes",
            "samples",
            "pcm_frames",
            "samplerate",
            "channels",
        ] {
            assert!(
                line.contains(&format!("\"{key}\"")),
                "missing {key} in {line}"
            );
        }
        assert!(!line.contains('\n'));
    }

    #[test]
    fn baseline_delta_nonnegative() {
        let b = Baseline::take();
        let (u, s) = b.cpu_delta();
        assert!(u >= 0.0 && s >= 0.0);
    }
}
