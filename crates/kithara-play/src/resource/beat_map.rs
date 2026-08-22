use std::{collections::HashMap, fmt};

use kithara_assets::AssetScope;
use kithara_audio::{
    AssetAxis, AssetBeatMap, AssetMapPublishError, AssetMapPublisher, AssetMapUpdate, BeatMap,
    BeatMapId, BeatMapSnapshot,
};
use kithara_platform::sync::{Arc, Mutex};

/// Off-audio-thread owner of the map identities in one session topology.
#[derive(Clone)]
pub struct AssetMapRegistry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    state: Mutex<RegistryState>,
}

struct RegistryState {
    assets: HashMap<Arc<str>, Arc<Mutex<AssetMapEntry>>>,
}

struct AssetMapEntry {
    axis: AssetAxis,
    map: AssetBeatMap,
    publisher: AssetMapPublisher,
    claimed: bool,
}

/// Producer lease containing one shared map and its exclusive publisher.
///
/// The orchestration owner must retain this lease for the complete producer
/// lifetime. Dropping it releases the publisher capability; read handles keep
/// the same map identity and receive revisions from a later producer.
#[non_exhaustive]
pub struct AssetMapRegistration {
    map: AssetBeatMap,
    lease: AssetMapLease,
}

struct AssetMapLease {
    entry: Arc<Mutex<AssetMapEntry>>,
}

/// An asset map could not be registered.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum AssetMapRegistryError {
    /// The same logical asset was registered with incompatible decoded axes.
    #[error("asset map axis conflicts with its registered axis")]
    AxisConflict {
        expected: AssetAxis,
        given: AssetAxis,
    },
    /// Another producer still owns this logical asset registration.
    #[error("asset map publisher is already claimed")]
    PublisherClaimed,
    /// The registry cannot issue another map identity without aliasing.
    #[error("beat map identity space is exhausted")]
    IdExhausted,
}

impl Default for AssetMapRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                state: Mutex::new(RegistryState {
                    assets: HashMap::new(),
                }),
            }),
        }
    }
}

impl fmt::Debug for AssetMapRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssetMapRegistry")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for AssetMapRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssetMapRegistration")
            .field("map", &self.map)
            .finish_non_exhaustive()
    }
}

impl BeatMap for AssetMapRegistration {
    delegate::delegate! {
        to self.map {
            fn id(&self) -> BeatMapId;
            fn snapshot(&self) -> BeatMapSnapshot;
        }
    }
}

impl AssetMapRegistration {
    /// Clones the shared read handle for a map consumer.
    #[must_use]
    pub fn map(&self) -> AssetBeatMap {
        self.map.clone()
    }

    /// Publishes the next validated revision through this exclusive lease.
    ///
    /// # Errors
    ///
    /// Returns [`AssetMapPublishError`] when the candidate violates the map
    /// revision, lifecycle, or geometry contract.
    pub fn publish(
        &mut self,
        update: AssetMapUpdate,
    ) -> Result<BeatMapSnapshot, AssetMapPublishError> {
        self.lease.entry.lock().publisher.publish(update)
    }
}

impl Drop for AssetMapLease {
    fn drop(&mut self) {
        self.entry.lock().claimed = false;
    }
}

impl AssetMapRegistry {
    /// Registers one layout-selected asset producer.
    ///
    /// The returned lease is the publisher capability and carries the read
    /// handle to distribute. Dropping it releases the publisher; a later
    /// producer resumes the same map identity and current revision.
    ///
    /// # Errors
    ///
    /// Returns [`AssetMapRegistryError::AxisConflict`] for an incompatible
    /// persisted registration, [`AssetMapRegistryError::PublisherClaimed`] when
    /// its producer is still alive, or [`AssetMapRegistryError::IdExhausted`].
    pub fn map(
        &self,
        scope: &AssetScope,
        axis: AssetAxis,
    ) -> Result<AssetMapRegistration, AssetMapRegistryError> {
        let key = Arc::<str>::from(scope.asset_root());
        let mut state = self.inner.state.lock();
        let (entry, map) = if let Some(entry) = state.assets.get(&key).cloned() {
            let mut stored = entry.lock();
            if stored.axis != axis {
                return Err(AssetMapRegistryError::AxisConflict {
                    expected: stored.axis,
                    given: axis,
                });
            }
            if stored.claimed {
                return Err(AssetMapRegistryError::PublisherClaimed);
            }
            stored.claimed = true;
            let map = stored.map.clone();
            drop(stored);
            (entry, map)
        } else {
            let id = BeatMapId::allocate().map_err(|_| AssetMapRegistryError::IdExhausted)?;
            let (map, publisher) = AssetBeatMap::new(id, axis.sample_rate(), axis.frame_count());
            let entry = Arc::new(Mutex::new(AssetMapEntry {
                axis,
                map: map.clone(),
                publisher,
                claimed: true,
            }));
            state.assets.insert(key, Arc::clone(&entry));
            (entry, map)
        };
        drop(state);
        Ok(AssetMapRegistration {
            map,
            lease: AssetMapLease { entry },
        })
    }

    pub(crate) fn reserve_host_id() -> Result<BeatMapId, AssetMapRegistryError> {
        BeatMapId::allocate().map_err(|_| AssetMapRegistryError::IdExhausted)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_assets::{AssetScope, AssetSource, AssetStore};
    use kithara_audio::{AssetAxis, AssetMapUpdate, BeatMap, MapState};
    use kithara_test_utils::kithara;
    use url::Url;

    use super::{AssetMapRegistry, AssetMapRegistryError};

    struct TestAsset;

    fn scope(store: &AssetStore) -> AssetScope {
        scope_at(store, "track.wav")
    }

    fn scope_at(store: &AssetStore, path: &str) -> AssetScope {
        store
            .scope::<TestAsset>(&AssetSource::Remote {
                url: Url::parse(&format!("https://example.com/{path}"))
                    .expect("invariant: fixture URL is valid"),
                discriminator: None,
            })
            .expect("invariant: fixture scope is valid")
    }

    fn axis() -> AssetAxis {
        AssetAxis::new(
            NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero"),
            96_000,
        )
    }

    #[kithara::test]
    fn host_and_asset_maps_share_one_identity_sequence() {
        let registry = AssetMapRegistry::default();
        let store = AssetStore::builder().build();
        let host = AssetMapRegistry::reserve_host_id()
            .expect("invariant: fresh registry can reserve a host identity");
        let asset = registry
            .map(&scope(&store), axis())
            .expect("invariant: fixture asset can be registered");

        assert_ne!(host, asset.id());
    }

    #[kithara::test]
    fn registration_distributes_one_map_and_owns_one_publisher() {
        let registry = AssetMapRegistry::default();
        let store = AssetStore::builder().build();
        let scope = scope(&store);
        let mut registration = registry
            .map(&scope, axis())
            .expect("invariant: first registration is valid");
        let first = registration.map();
        let second = registration.map();
        let before = first.snapshot();
        let published = registration
            .publish(AssetMapUpdate::new(
                before.stamp(),
                MapState::Building,
                Vec::new(),
            ))
            .expect("invariant: empty building refinement is valid");

        assert_eq!(second.snapshot().stamp(), published.stamp());
        assert!(matches!(
            registry.map(&scope, axis()),
            Err(AssetMapRegistryError::PublisherClaimed)
        ));
    }

    #[kithara::test]
    fn dropping_a_producer_preserves_identity_and_releases_its_publisher() {
        let registry = AssetMapRegistry::default();
        let store = AssetStore::builder().build();
        let scope = scope(&store);
        let abandoned = registry
            .map(&scope, axis())
            .expect("invariant: registration is valid");
        let reader = abandoned.map();
        let abandoned_id = abandoned.id();
        drop(abandoned);

        let mut replacement = registry
            .map(&scope, axis())
            .expect("invariant: released publisher can be claimed again");
        let published = replacement
            .publish(AssetMapUpdate::new(
                reader.snapshot().stamp(),
                MapState::Building,
                Vec::new(),
            ))
            .expect("invariant: replacement producer can publish a refinement");

        assert_eq!(abandoned_id, replacement.id());
        assert_eq!(reader.snapshot().stamp(), published.stamp());
    }

    #[kithara::test]
    fn swapping_complete_leases_preserves_each_registry_entry() {
        let registry = AssetMapRegistry::default();
        let store = AssetStore::builder().build();
        let first_scope = scope_at(&store, "first.wav");
        let second_scope = scope_at(&store, "second.wav");
        let mut first = registry
            .map(&first_scope, axis())
            .expect("invariant: first registration is valid");
        let mut second = registry
            .map(&second_scope, axis())
            .expect("invariant: second registration is valid");
        let first_id = first.id();
        let second_id = second.id();

        std::mem::swap(&mut first, &mut second);
        drop(first);
        drop(second);

        let first = registry
            .map(&first_scope, axis())
            .expect("invariant: first publisher returned to its entry");
        let second = registry
            .map(&second_scope, axis())
            .expect("invariant: second publisher returned to its entry");
        assert_eq!(first.id(), first_id);
        assert_eq!(second.id(), second_id);
    }
}
