/// A value kept beside the key it was built from.
///
/// The guard belongs at the call site: the point of this cache is that nothing
/// is prepared for a hit, so it holds no builder and offers no way to hand it
/// one. A caller asks whether the key still holds, and only then builds.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct CachedValue<K: PartialEq + Default, V> {
    #[field(get, vis = "pub(crate)")]
    key: K,
    value: Option<V>,
}

impl<K: PartialEq + Default, V> Default for CachedValue<K, V> {
    fn default() -> Self {
        Self {
            key: K::default(),
            value: None,
        }
    }
}

impl<K: PartialEq + Default, V> CachedValue<K, V> {
    pub(crate) fn value(&self) -> Option<&V> {
        self.value.as_ref()
    }

    /// Takes the new pair when the old one no longer answers. A key that still
    /// holds keeps the value it was paired with, so a caller that re-derived
    /// the same key cannot replace a good value with a stale one.
    pub(crate) fn update(&mut self, key: K, value: Option<V>) {
        if self.value.is_none() || self.key != key {
            self.key = key;
            self.value = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::CachedValue;

    #[kithara::test]
    fn a_fresh_cache_holds_no_value() {
        let cached = CachedValue::<u8, &str>::default();

        assert_eq!(cached.value(), None, "nothing was ever built");
    }

    #[kithara::test]
    fn a_new_key_takes_the_value_that_came_with_it() {
        let mut cached = CachedValue::default();

        cached.update(1_u8, Some("one"));

        assert_eq!(cached.value(), Some(&"one"));
    }

    #[kithara::test]
    fn the_key_moves_with_the_value_it_admitted() {
        let mut cached = CachedValue::default();

        cached.update(1_u8, Some("one"));

        assert_eq!(cached.key(), &1, "the value is only as good as its key");
    }

    #[kithara::test]
    fn a_key_that_still_holds_keeps_its_value() {
        let mut cached = CachedValue::default();
        cached.update(1_u8, Some("one"));

        cached.update(1, None);

        assert_eq!(cached.value(), Some(&"one"), "the key did not move");
    }

    #[kithara::test]
    fn a_moved_key_takes_the_value_offered_with_it() {
        let mut cached = CachedValue::default();
        cached.update(1_u8, Some("one"));

        cached.update(2, Some("two"));

        assert_eq!(cached.value(), Some(&"two"));
    }
}
