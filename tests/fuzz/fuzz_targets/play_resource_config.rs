#![no_main]

use std::sync::LazyLock;

use arbitrary::Arbitrary;
use kithara::{
    assets::{AssetStore, StorageBackend},
    play::{PlaybackResamplerBackend, ResourceConfig},
};
use libfuzzer_sys::fuzz_target;

static STORE: LazyLock<AssetStore> = LazyLock::new(|| {
    AssetStore::builder()
        .backend(StorageBackend::Memory)
        .build()
});

#[derive(Arbitrary, Debug)]
struct Input {
    raw: Vec<u8>,
}

fuzz_target!(|input: Input| {
    let mut raw = input.raw;
    raw.truncate(4 * 1024);

    let text = String::from_utf8_lossy(&raw);
    let _ = ResourceConfig::parse_src(text.as_ref()).map(|src| {
        ResourceConfig::<PlaybackResamplerBackend>::for_src(src)
            .store(STORE.clone())
            .build()
    });
});
