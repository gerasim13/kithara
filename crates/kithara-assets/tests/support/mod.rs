use std::sync::atomic::{AtomicUsize, Ordering};

use kithara_assets::{AssetResource, AssetSource, ChunkSink, ProcessCtx, ResourceProcessor};
use kithara_platform::sync::Arc;
use url::Url;

pub(crate) mod pools;

pub(crate) use pools::pools;

pub(crate) struct Test;

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

pub(crate) fn xor_processor(key: u8, calls: Option<Arc<AtomicUsize>>) -> ProcessCtx {
    Arc::new(XorProcessor {
        calls,
        identity: [key],
        key,
    })
}

pub(crate) fn source(identity: &str) -> AssetSource {
    AssetSource::Remote {
        url: Url::parse("https://assets.test.invalid/track").expect("valid test source URL"),
        discriminator: Some(identity.to_owned()),
    }
}

pub(crate) fn resource(raw: impl AsRef<str>) -> AssetResource {
    let raw = raw.as_ref();
    let (namespace, name) = raw.split_once('/').unwrap_or(("resource", raw));
    AssetResource::Named {
        namespace: namespace.to_owned(),
        name: name.to_owned(),
    }
}
