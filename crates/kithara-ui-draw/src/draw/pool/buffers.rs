use kithara_bufpool::{
    HasPool, OverallBudget, Pool, PoolAlias, PoolConfig, PoolError, PoolRegion, StringKey, VecKey,
    pool_schema,
};

use super::{
    super::{DrawCmd, DrawListBuilder, FillRule, PoolPath, Verb},
    Buffer, PoolText,
};
use crate::DrawPoolLimits;

mod consts {
    pub(super) const SHARDS: usize = 1;
}

/// Registered slot for draw-list commands in an application's region.
pub type DrawCommandKey = PoolAlias<DrawCmd, VecKey<DrawCmd, { consts::SHARDS }>>;
/// Registered slot for vector-path verbs in an application's region.
pub type DrawPathKey = PoolAlias<Verb, VecKey<Verb, { consts::SHARDS }>>;
/// Registered slot for UTF-8 draw text in an application's region.
pub type DrawTextKey = PoolAlias<PoolText, StringKey<{ consts::SHARDS }>>;

pool_schema! {
    pub(crate) DrawSchema {
        commands: DrawCommandKey,
        paths: DrawPathKey,
        text: DrawTextKey,
    }
}

/// Aggregate reuse statistics for every draw buffer kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Acquisitions that had to create a fresh empty buffer.
    pub alloc_misses: u64,
    /// Acquisitions served by the caller thread's shard.
    pub home_hits: u64,
    /// Returned buffers dropped because their shard was full or rejected them.
    pub put_drops: u64,
    /// Acquisitions served by a different thread shard.
    pub steal_hits: u64,
}

/// Shared reusable storage for retained commands, paths, and text.
#[derive(Clone, Debug)]
pub struct DrawBuffers {
    commands: Pool<DrawCommandKey>,
    limits: DrawPoolLimits,
    paths: Pool<DrawPathKey>,
    text: Pool<DrawTextKey>,
}

impl DrawBuffers {
    /// Builds the registered draw-buffer family under one shared hard budget.
    ///
    /// # Panics
    ///
    /// Panics if the internally generated schema configuration is invalid. Only
    /// [`DrawPoolLimits::default`] was ever handed to this until a configuration
    /// document could name one; a caller building from document-supplied values
    /// should use [`Self::try_new`] instead, which reports the same failure
    /// rather than aborting.
    #[must_use]
    pub fn new(limits: DrawPoolLimits) -> Self {
        match Self::try_new(limits) {
            Ok(buffers) => buffers,
            Err(error) => panic!("valid draw buffer configuration failed: {error}"),
        }
    }

    pub(in crate::draw) fn commands(&self) -> Buffer<DrawCmd> {
        Buffer::pooled(self.commands.get())
    }

    #[must_use]
    pub const fn limits(&self) -> DrawPoolLimits {
        self.limits
    }

    /// Starts an empty command list backed by this buffer family.
    #[must_use]
    pub fn list(&self) -> DrawListBuilder {
        DrawListBuilder::pooled(self)
    }

    /// Copies path verbs into a buffer that returns here when unused.
    #[must_use]
    pub fn path<Verbs>(&self, rule: FillRule, verbs: Verbs) -> PoolPath
    where
        Verbs: IntoIterator<Item = Verb>,
    {
        let mut path = PoolPath::pooled(rule, self.paths.get());
        path.extend(verbs);
        path
    }

    pub(in crate::draw) fn pooled_path(&self, path: PoolPath) -> PoolPath {
        path.into_pooled(|| self.paths.get())
    }

    #[must_use]
    pub fn stats(&self) -> PoolStats {
        let stats = [self.commands.stats(), self.paths.stats(), self.text.stats()];
        PoolStats {
            alloc_misses: stats.iter().map(|stats| stats.alloc_misses).sum(),
            home_hits: stats.iter().map(|stats| stats.home_hits).sum(),
            put_drops: stats.iter().map(|stats| stats.put_drops).sum(),
            steal_hits: stats.iter().map(|stats| stats.steal_hits).sum(),
        }
    }

    /// Copies UTF-8 content into a buffer that returns here when unused.
    #[must_use]
    pub fn text(&self, content: &str) -> PoolText {
        PoolText::pooled(content, self.text.get())
    }

    /// Use the draw slots already registered in the application's region.
    ///
    /// The application's schema configures the three slots' retention policies
    /// from `limits`. This constructor allocates no region or payload; commands,
    /// paths and text share the supplied region's hard cap with its other slots.
    /// The reported byte limit always reflects that originating region's cap.
    #[must_use]
    pub fn from_region<S>(mut limits: DrawPoolLimits, region: &PoolRegion<S>) -> Self
    where
        S: HasPool<DrawCommandKey> + HasPool<DrawPathKey> + HasPool<DrawTextKey>,
    {
        limits.max_bytes = region.stats().max_bytes;
        Self {
            commands: region.pool::<DrawCommandKey>(),
            limits,
            paths: region.pool::<DrawPathKey>(),
            text: region.pool::<DrawTextKey>(),
        }
    }

    /// Builds the registered draw-buffer family under one shared hard budget.
    ///
    /// # Errors
    /// Returns the [`PoolError`] the generated schema configuration failed
    /// with -- reachable once `limits` comes from a configuration document
    /// rather than only ever [`DrawPoolLimits::default`].
    pub fn try_new(limits: DrawPoolLimits) -> Result<Self, PoolError> {
        let max_buffers = limits.max_buffers.max(1);
        let config = |max_retained_capacity| {
            PoolConfig::builder()
                .max_buffers(max_buffers)
                .max_retained_capacity(max_retained_capacity)
                .build()
        };
        let region = DrawSchema::builder(OverallBudget(limits.max_bytes))
            .commands(config(limits.command_capacity))
            .paths(config(limits.path_capacity))
            .text(config(limits.text_capacity))
            .build()?;
        Ok(Self::from_region(limits, &region))
    }
}

impl Default for DrawBuffers {
    fn default() -> Self {
        Self::new(DrawPoolLimits::default())
    }
}

impl PartialEq for DrawBuffers {
    fn eq(&self, other: &Self) -> bool {
        self.limits == other.limits
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::draw::{Pt, Rect, Rgba};

    pool_schema! {
        AppDrawPools {
            commands: DrawCommandKey,
            paths: DrawPathKey,
            text: DrawTextKey,
            bytes: u8,
        }
    }

    #[kithara::test]
    fn injected_draw_slots_share_admission_and_reuse_with_the_application() {
        const BUDGET: usize = 1024;
        let limits = DrawPoolLimits::builder()
            .max_buffers(1)
            .command_capacity(8)
            .path_capacity(8)
            .text_capacity(32)
            .build();
        let config = |capacity| {
            PoolConfig::builder()
                .max_buffers(limits.max_buffers)
                .max_retained_capacity(capacity)
                .build()
        };
        let region = AppDrawPools::builder(OverallBudget(BUDGET))
            .commands(config(limits.command_capacity))
            .paths(config(limits.path_capacity))
            .text(config(limits.text_capacity))
            .bytes(PoolConfig::builder().max_buffers(32).build())
            .build()
            .unwrap_or_else(|error| panic!("application region: {error}"));
        let buffers = DrawBuffers::from_region(limits, &region);
        let held = buffers.text("held");
        let misses = region.pool_stats::<DrawTextKey>().alloc_misses;

        assert_eq!(buffers.limits().max_bytes, BUDGET);
        assert!(matches!(
            region.get_with_len::<u8>(BUDGET),
            Err(PoolError::OverallBudgetExceeded { .. })
        ));
        assert_eq!(held.as_str(), "held");
        drop(held);
        drop(buffers.clone().text("back"));
        assert_eq!(region.pool_stats::<DrawTextKey>().alloc_misses, misses);
        assert!(region.pool_stats::<DrawTextKey>().home_hits >= 1);

        let bytes = region
            .get_with_len::<u8>(BUDGET)
            .unwrap_or_else(|error| panic!("idle draw capacity must be reclaimable: {error}"));
        assert_eq!(region.stats().allocated_bytes, BUDGET);
        drop(bytes);
        drop(buffers.path(FillRule::NonZero, [Verb::Close]));
        assert!(region.stats().allocated_bytes <= BUDGET);
    }

    #[kithara::test]
    fn released_draw_values_are_reused_and_observable() {
        let buffers = DrawBuffers::new(
            DrawPoolLimits::builder()
                .max_buffers(4)
                .command_capacity(8)
                .path_capacity(8)
                .text_capacity(16)
                .build(),
        );
        {
            let path = buffers.path(FillRule::NonZero, [Verb::MoveTo(Pt { x: 0.0, y: 0.0 })]);
            let text = buffers.text("pooled");
            assert_eq!(text.as_str(), "pooled");
            let mut list = buffers.list();
            list.fill_path(
                path,
                Rgba {
                    a: 1.0,
                    b: 0.0,
                    g: 0.0,
                    r: 1.0,
                },
            );
            list.clip(
                Rect {
                    h: 1.0,
                    w: 1.0,
                    x: 0.0,
                    y: 0.0,
                },
                buffers.list().finish(),
            );
            drop(list.finish());
        }
        let cold = buffers.stats().alloc_misses;
        let cold_by_kind = [
            buffers.commands.stats().alloc_misses,
            buffers.paths.stats().alloc_misses,
            buffers.text.stats().alloc_misses,
        ];
        {
            let path = buffers.path(FillRule::NonZero, [Verb::Close]);
            let text = buffers.text("again");
            assert_eq!(text.as_str(), "again");
            let mut list = buffers.list();
            list.fill_path(
                path,
                Rgba {
                    a: 1.0,
                    b: 0.0,
                    g: 0.0,
                    r: 1.0,
                },
            );
            drop(list.finish());
        }

        assert!(
            cold >= 4,
            "each empty buffer kind must report its first allocation"
        );
        assert_eq!(
            [
                buffers.commands.stats().alloc_misses,
                buffers.paths.stats().alloc_misses,
                buffers.text.stats().alloc_misses,
            ],
            cold_by_kind
        );
        assert_eq!(buffers.stats().alloc_misses, cold);
        assert!(buffers.stats().home_hits >= 3);
    }

    #[kithara::test]
    fn retained_snapshots_do_not_sequester_reusable_buffers() {
        let buffers = DrawBuffers::new(
            DrawPoolLimits::builder()
                .max_buffers(4)
                .command_capacity(8)
                .path_capacity(8)
                .text_capacity(16)
                .build(),
        );
        let text = buffers.text("cached");
        let mut list = buffers.list();
        list.fill_path(
            buffers.path(FillRule::NonZero, [Verb::Close]),
            Rgba {
                a: 1.0,
                b: 1.0,
                g: 1.0,
                r: 1.0,
            },
        );
        let list = list.finish();
        let before = buffers.stats().alloc_misses;

        let snapshot = list.clone();
        let text_snapshot = text.clone();

        assert_eq!(buffers.stats().alloc_misses, before);
        assert_eq!(snapshot, list);
        assert_eq!(text_snapshot, text);
    }

    #[kithara::test]
    fn oversized_buffers_are_dropped_without_truncating_live_values() {
        let buffers = DrawBuffers::new(
            DrawPoolLimits::builder()
                .max_buffers(1)
                .command_capacity(1)
                .path_capacity(2)
                .text_capacity(1)
                .build(),
        );
        let path = buffers.path(
            FillRule::NonZero,
            [
                Verb::MoveTo(Pt { x: 0.0, y: 0.0 }),
                Verb::LineTo(Pt { x: 1.0, y: 0.0 }),
                Verb::Close,
            ],
        );
        assert_eq!(path.verbs().len(), 3);
        drop(path);
        let after_drop = buffers.stats();
        assert_eq!(after_drop.put_drops, 1);

        drop(buffers.path(FillRule::NonZero, [Verb::Close]));
        assert_eq!(buffers.stats().alloc_misses, after_drop.alloc_misses + 1);
    }
}
