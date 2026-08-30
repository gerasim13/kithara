use std::marker::PhantomData;

use crate::{ByteBuffer, PoolConfig, PoolError, SampleBuffer, pool::Core, region::BuildContext};

mod sealed {
    pub trait Sealed {}
}

/// Unforgeable capability for crate-owned key operations.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PoolAccess {
    _private: AccessToken,
}

#[derive(Clone, Copy)]
struct AccessToken;

impl PoolAccess {
    pub(crate) const fn new() -> Self {
        Self {
            _private: AccessToken,
        }
    }
}

/// A compile-time key for one physical pool in a closed schema.
pub trait PoolKey: sealed::Sealed {
    /// Element stored in this pool's buffers.
    type Item;
    /// Nominal checked guard returned by the facade.
    type Buffer;
    /// Opaque core used by schema-generated plumbing.
    #[doc(hidden)]
    type Core;

    /// Build one physical slot.
    #[doc(hidden)]
    fn __build(
        context: &BuildContext,
        config: PoolConfig,
        access: PoolAccess,
    ) -> Result<Self::Core, PoolError>;

    /// Acquire an empty buffer from the slot.
    #[doc(hidden)]
    fn __get(core: &Self::Core, access: PoolAccess) -> Self::Buffer;

    /// Acquire a buffer and grow it to `len` elements.
    #[doc(hidden)]
    fn __get_with_len(
        core: &Self::Core,
        len: usize,
        access: PoolAccess,
    ) -> Result<Self::Buffer, PoolError>;
}

/// A distinct physical pool that reuses another key's element and guard policy.
pub struct PoolAlias<Tag, K>(PhantomData<Tag>, PhantomData<K>);

impl<Tag, K> sealed::Sealed for PoolAlias<Tag, K> where K: PoolKey {}

impl<Tag, K> PoolKey for PoolAlias<Tag, K>
where
    K: PoolKey,
{
    type Buffer = K::Buffer;
    type Core = K::Core;
    type Item = K::Item;

    fn __build(
        context: &BuildContext,
        config: PoolConfig,
        access: PoolAccess,
    ) -> Result<Self::Core, PoolError> {
        K::__build(context, config, access)
    }

    fn __get(core: &Self::Core, access: PoolAccess) -> Self::Buffer {
        K::__get(core, access)
    }

    fn __get_with_len(
        core: &Self::Core,
        len: usize,
        access: PoolAccess,
    ) -> Result<Self::Buffer, PoolError> {
        K::__get_with_len(core, len, access)
    }
}

/// Opaque core for the built-in byte key.
#[doc(hidden)]
pub struct ByteCore(kithara_platform::sync::Arc<Core<32, u8>>);

/// Opaque core for the built-in decoded-sample key.
#[doc(hidden)]
pub struct SampleCore(kithara_platform::sync::Arc<Core<8, f32>>);

impl sealed::Sealed for u8 {}

impl PoolKey for u8 {
    type Buffer = ByteBuffer;
    type Core = ByteCore;
    type Item = Self;

    fn __build(
        context: &BuildContext,
        config: PoolConfig,
        _access: PoolAccess,
    ) -> Result<Self::Core, PoolError> {
        let limit = context.pool_limit(config.max_share)?;
        Core::new(config, context.region_budget(), limit)
            .map(kithara_platform::sync::Arc::new)
            .map(ByteCore)
    }

    fn __get(core: &Self::Core, _access: PoolAccess) -> Self::Buffer {
        ByteBuffer::new(core.0.acquire())
    }

    fn __get_with_len(
        core: &Self::Core,
        len: usize,
        access: PoolAccess,
    ) -> Result<Self::Buffer, PoolError> {
        let mut buffer = Self::__get(core, access);
        buffer.ensure_len(len)?;
        Ok(buffer)
    }
}

impl sealed::Sealed for f32 {}

impl PoolKey for f32 {
    type Buffer = SampleBuffer;
    type Core = SampleCore;
    type Item = Self;

    fn __build(
        context: &BuildContext,
        config: PoolConfig,
        _access: PoolAccess,
    ) -> Result<Self::Core, PoolError> {
        let limit = context.pool_limit(config.max_share)?;
        Core::new(config, context.region_budget(), limit)
            .map(kithara_platform::sync::Arc::new)
            .map(SampleCore)
    }

    fn __get(core: &Self::Core, _access: PoolAccess) -> Self::Buffer {
        SampleBuffer::new(core.0.acquire())
    }

    #[inline]
    fn __get_with_len(
        core: &Self::Core,
        len: usize,
        access: PoolAccess,
    ) -> Result<Self::Buffer, PoolError> {
        let mut buffer = Self::__get(core, access);
        buffer.ensure_len(len)?;
        Ok(buffer)
    }
}
