#![forbid(unsafe_code)]

/// AES-128-CBC decryption context.
///
/// Carried as a per-acquire `ProcessCtx` trait object
/// when decrypting a resource on commit.
#[derive(Clone, Default, derive_more::Debug, Hash, PartialEq, Eq)]
pub struct DecryptContext {
    /// AES-128 key (16 bytes).
    #[debug("<redacted>")]
    pub key: [u8; Self::KEY_LEN_128],
    /// Initialization vector (16 bytes).
    #[debug("<redacted>")]
    pub iv: [u8; Self::IV_LEN],
}

impl DecryptContext {
    /// AES initialization vector length in bytes.
    const IV_LEN: usize = 16;

    /// AES-128 key length in bytes.
    const KEY_LEN_128: usize = 16;

    /// Create a new decryption context.
    #[must_use]
    pub const fn new(key: [u8; Self::KEY_LEN_128], iv: [u8; Self::IV_LEN]) -> Self {
        Self { key, iv }
    }
}
