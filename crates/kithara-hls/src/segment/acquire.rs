use kithara_assets::{AssetStore, AssetsResult, ResourceAcquisition};
use kithara_drm::as_process_ctx;

use super::{Segment, SegmentContent};

impl Segment {
    pub(crate) fn acquire(&self, store: &AssetStore) -> AssetsResult<ResourceAcquisition> {
        match self.content() {
            SegmentContent::Plain => store.acquire_resource(self.resource_id(), None),
            SegmentContent::Encrypted(context) => store.acquire_resource_with_ctx(
                self.resource_id(),
                None,
                Some(as_process_ctx(context.clone())),
            ),
        }
    }
}
