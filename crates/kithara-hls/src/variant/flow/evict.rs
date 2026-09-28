use kithara_assets::ResourceKey;
use kithara_bufpool::HasPool;
use kithara_test_utils::kithara;

use crate::variant::HlsVariant;

impl<S> HlsVariant<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Returns evicted `seg_idx` (`-1` for init), or `None` if `key` doesn't belong to this variant.
    /// The slot ends `Missing`, through its claim if a fetch is in flight; queue reseeding is the
    /// caller's job (see `HlsCoord::broadcast_eviction` for resident reader sessions; variants
    /// without a reader are rebuilt lazily on the next ABR flip).
    #[kithara::probe(variant = self.variant as u64)]
    pub(crate) fn on_evict(&self, key: &ResourceKey) -> Option<i32> {
        self.segments.release(key);
        if let Some(init) = self.segments.init.as_ref()
            && init.resource_id() == key
        {
            init.state().mark_evicted();
            return Some(-1);
        }
        let (seg_idx, seg) = self
            .segments
            .iter()
            .enumerate()
            .find(|(_, seg)| seg.resource_id() == key)?;
        seg.state().mark_evicted();
        i32::try_from(seg_idx).ok()
    }
}
