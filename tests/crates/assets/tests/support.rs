#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use kithara::{
    assets::{
        AcquisitionResult, AssetLayout, AssetLayoutRegistry, AssetResource, AssetScope,
        AssetSource, AssetStore, ChunkSink, ProcessCtx, ResourceProcessor, StorageBackend,
        WriteSide,
    },
    platform::sync::Arc,
};
use kithara_integration_tests::{
    TestTempDir,
    bufpool_ext::{TestPools, pools},
};
use url::Url;

const RESOURCE_NAMESPACE: &str = "test-resource";

#[derive(Debug)]
struct XorProcessor {
    calls: Option<Arc<AtomicUsize>>,
    identity: [u8; 1],
    key: u8,
}

impl ResourceProcessor for XorProcessor {
    fn begin(&self) -> Box<dyn ChunkSink> {
        Box::new(XorSink {
            calls: self.calls.clone(),
            key: self.key,
        })
    }

    fn identity(&self) -> &[u8] {
        &self.identity
    }
}

struct XorSink {
    calls: Option<Arc<AtomicUsize>>,
    key: u8,
}

impl ChunkSink for XorSink {
    fn process(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        _is_last: bool,
    ) -> Result<usize, String> {
        if let Some(calls) = &self.calls {
            calls.fetch_add(1, Ordering::SeqCst);
        }
        for (output, input) in output.iter_mut().zip(input) {
            *output = *input ^ self.key;
        }
        Ok(input.len())
    }
}

pub(super) fn xor_processor(key: u8, calls: Option<Arc<AtomicUsize>>) -> ProcessCtx {
    Arc::new(XorProcessor {
        calls,
        identity: [key],
        key,
    })
}

#[derive(Debug)]
pub(super) struct LiteralLayout;

impl AssetLayout for LiteralLayout {
    fn root(&self, source: &AssetSource) -> String {
        let AssetSource::Remote {
            discriminator: Some(root),
            ..
        } = source
        else {
            panic!("literal test layout requires an explicit root")
        };
        root.clone()
    }

    fn path(&self, resource: &AssetResource) -> String {
        let AssetResource::Named { namespace, name } = resource else {
            panic!("literal test layout requires a named resource")
        };
        assert_eq!(namespace, RESOURCE_NAMESPACE);
        name.clone()
    }
}

pub(super) fn literal_layouts() -> AssetLayoutRegistry {
    AssetLayoutRegistry::new(Arc::new(LiteralLayout))
}

pub(super) fn source(asset_root: &str) -> AssetSource {
    AssetSource::Remote {
        url: Url::parse("https://cache.test/source").expect("valid test URL"),
        discriminator: Some(asset_root.to_string()),
    }
}

pub(super) fn resource(path: impl Into<String>) -> AssetResource {
    AssetResource::Named {
        namespace: RESOURCE_NAMESPACE.to_string(),
        name: path.into(),
    }
}

pub(super) fn pending<W: WriteSide>(acquisition: AcquisitionResult<W, W::Reader>) -> W {
    let AcquisitionResult::Pending(writer) = acquisition else {
        panic!("expected a Pending writer")
    };
    writer
}

pub(super) fn asset_scope(temp_dir: &TestTempDir, asset_root: &str) -> AssetScope<TestPools> {
    #[cfg(not(target_arch = "wasm32"))]
    let backend = StorageBackend::Disk {
        root: temp_dir.path().into(),
    };
    #[cfg(target_arch = "wasm32")]
    let backend = {
        let _ = temp_dir;
        StorageBackend::Memory
    };

    AssetStore::builder(pools())
        .backend(backend)
        .layouts(literal_layouts())
        .build()
        .scope::<LiteralLayout>(&source(asset_root))
        .expect("scope")
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn asset_dir_exists(root: &Path, asset_root: &str) -> bool {
    root.join(asset_root).exists()
}
