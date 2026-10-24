use super::ClientBuilder;
use crate::types::Compression;

/// A native compression transform. Request code owns the exact header because
/// browser emulation may add one independently of the enabled decoders.
type ClientBuilderMod = fn(ClientBuilder) -> ClientBuilder;

pub(crate) fn apply_compression(builder: ClientBuilder, c: Compression) -> ClientBuilder {
    [
        (
            Compression::GZIP,
            ClientBuilder::no_gzip as ClientBuilderMod,
        ),
        (Compression::DEFLATE, ClientBuilder::no_deflate),
        (Compression::BROTLI, ClientBuilder::no_brotli),
        (Compression::ZSTD, ClientBuilder::no_zstd),
    ]
    .into_iter()
    .filter(|(flag, _)| !c.contains(*flag))
    .fold(builder, |b, (_, disable)| disable(b))
}
