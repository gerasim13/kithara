#![forbid(unsafe_code)]

use crate::sync::Arc;

/// Values a writer displaced from an `ArcSwap`, held until quiesced.
///
/// A swap pays every reader debt on the displaced value before it returns,
/// so once this list is a value's only owner no reader guard resolves to it
/// and none can. Holding displaced values here makes every reader guard drop
/// a pure decrement: frees run only in [`retire`](Self::retire), on the
/// writer, whose `&mut` borrow is its serialization.
#[derive_where::derive_where(Default)]
pub struct Retired<T> {
    displaced: Vec<Arc<T>>,
}

impl<T> Retired<T> {
    /// Free every quiesced value, then hold `displaced` until a later call
    /// finds it quiesced.
    pub fn retire(&mut self, displaced: Arc<T>) {
        self.displaced.retain(|value| Arc::strong_count(value) > 1);
        self.displaced.push(displaced);
    }
}

#[cfg(test)]
mod tests {
    use arc_swap::ArcSwap;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::sync::Weak;

    /// A reader guard held across retires is never the last owner of the value
    /// they displaced: its drop leaves the value to the list, and the first
    /// retire after that drop frees it.
    #[kithara::test]
    fn displaced_value_is_freed_by_the_writer() {
        let published = ArcSwap::from_pointee(0_u8);
        let mut retired = Retired::default();
        let reader = published.load();
        let displaced: Weak<u8> = Arc::downgrade(&reader);

        retired.retire(published.swap(Arc::new(1)));
        retired.retire(published.swap(Arc::new(2)));
        drop(reader);
        assert!(
            displaced.upgrade().is_some(),
            "a reader guard drop never frees a displaced value"
        );

        retired.retire(published.swap(Arc::new(3)));
        assert!(
            displaced.upgrade().is_none(),
            "the next retire frees a quiesced value"
        );
    }
}
