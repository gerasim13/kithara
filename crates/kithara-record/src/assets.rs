use std::error::Error as StdError;

use kithara_assets::{
    AcquisitionResult, AssetReader, AssetStore, AssetWriter, AssetsError, ResourceKey, WriteSide,
};
use kithara_bufpool::HasPool;
use thiserror::Error;

use crate::RecordingSink;

/// `RecordingSink` adapter for one canonical `AssetStore` resource transaction.
#[derive(Debug)]
pub struct AssetPartSink<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    store: AssetStore<S>,
    writer: Option<AssetWriter<S>>,
    key: ResourceKey,
}

impl<S> AssetPartSink<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Acquire a new pending asset resource as a recording transaction.
    ///
    /// # Errors
    /// Returns an assets error or rejects an already committed resource.
    pub fn acquire(store: &AssetStore<S>, key: &ResourceKey) -> Result<Self, AssetPartSinkError> {
        match store.acquire_resource(key, None)? {
            AcquisitionResult::Pending(writer) => Ok(Self {
                key: key.clone(),
                store: store.clone(),
                writer: Some(writer),
            }),
            AcquisitionResult::Ready(_) => Err(AssetPartSinkError::AlreadyCommitted),
            _ => Err(AssetPartSinkError::UnexpectedAcquisition),
        }
    }

    fn storage<E>(error: E) -> AssetPartSinkError
    where
        E: StdError + Send + Sync + 'static,
    {
        AssetPartSinkError::Storage(Box::new(error))
    }
}

impl<S> RecordingSink for AssetPartSink<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    type Error = AssetPartSinkError;
    type Output = AssetReader<S>;

    fn abort(&mut self) {
        if self.writer.take().is_none() {
            return;
        }
        if let Err(error) = self.store.remove_resource(&self.key) {
            tracing::warn!(%error, key = ?self.key, "recording asset rollback failed");
        }
    }

    fn commit(&mut self, final_len: u64) -> Result<Self::Output, Self::Error> {
        self.writer
            .take()
            .ok_or(AssetPartSinkError::Closed)?
            .commit(Some(final_len))
            .map_err(Self::storage)
    }

    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Self::Error> {
        self.writer
            .as_ref()
            .ok_or(AssetPartSinkError::Closed)?
            .write_at(offset, bytes)
            .map_err(Self::storage)
    }
}

/// Failure while opening or operating an `AssetStore` recording sink.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AssetPartSinkError {
    /// Asset acquisition failed.
    #[error(transparent)]
    Assets(#[from] AssetsError),
    /// The target already contains a committed resource.
    #[error("recording asset is already committed")]
    AlreadyCommitted,
    /// The transaction has already committed or aborted.
    #[error("recording asset transaction is closed")]
    Closed,
    /// A newer assets acquisition phase is not supported by this adapter.
    #[error("recording asset returned an unsupported acquisition phase")]
    UnexpectedAcquisition,
    /// Backing write or commit failed.
    #[error("recording asset storage failed: {0}")]
    Storage(#[source] Box<dyn StdError + Send + Sync>),
}
