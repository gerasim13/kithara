use kithara_config::{ConfigOwner, UpdatableConfig};

use crate::{Batch, Protocol, Sender, Seq, When};

/// A configuration an owner keeps live for a real-time executor.
///
/// The owner commits each change through the configuration's own check and
/// reads the committed value at once through [`ConfigOwner::config`]. Once
/// per pass it sends the latest value whole, for the executor's next block;
/// a change refused by a full channel stays pending for the next pass. A new
/// executor starts from [`Live::seed`]. There is no mutable borrow: every
/// change goes through [`Live::update`], which marks it for sending.
#[derive(Debug, ConfigOwner)]
#[config_owner(value)]
pub struct Live<C: UpdatableConfig + Copy> {
    value: C,
    pending: bool,
}

impl<C: UpdatableConfig + Copy> Live<C> {
    /// A live configuration whose executor already holds `value`.
    #[must_use]
    pub const fn new(value: C) -> Self {
        Self {
            value,
            pending: false,
        }
    }

    /// Sends the committed value, wrapped by `wrap` into one command, for the
    /// executor's next block, when a change is pending.
    ///
    /// Returns the batch's number, or `None` when nothing is pending or the
    /// channel is full; a full channel keeps the change pending.
    pub fn flush<P: Protocol, W: FnOnce(C) -> P::Command>(
        &mut self,
        sender: &mut Sender<P>,
        wrap: W,
    ) -> Option<Seq> {
        if !self.pending {
            return None;
        }
        let batch = Batch {
            basis: Vec::new(),
            commands: vec![wrap(self.value)],
        };
        let seq = sender.send(When::Next, batch).ok()?;
        self.pending = false;
        Some(seq)
    }

    /// The value a new executor starts from; nothing stays pending for it.
    pub fn seed(&mut self) -> C {
        self.pending = false;
        self.value
    }

    /// Commits `update` through the configuration's check and marks the
    /// value for the next [`Live::flush`].
    ///
    /// # Errors
    ///
    /// Returns the configuration's refusal; the value and what is pending
    /// stay as they were.
    pub fn update(&mut self, update: C::Update) -> Result<(), C::Error> {
        UpdatableConfig::apply_update(&mut self.value, update)?;
        self.pending = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use kithara_config::{Config, ConfigOwner};
    use kithara_test_utils::kithara;

    use super::Live;
    use crate::{ChannelConfig, Clock, Inbox, Protocol, Sender, Target, channel};

    #[derive(Clone, Copy, Debug, PartialEq, Config)]
    #[config(update, patch(validate = Self::audible, error = Silence))]
    struct Gain {
        #[config(value, update)]
        level: f32,
    }

    #[derive(Debug, PartialEq)]
    struct Silence;

    impl Gain {
        fn audible(self) -> Result<Self, Silence> {
            if self.level > 0.0 {
                Ok(self)
            } else {
                Err(Silence)
            }
        }
    }

    #[derive(Debug)]
    enum Test {}

    #[derive(Clone, Copy, Debug)]
    enum NoTarget {}

    impl Target for NoTarget {
        fn index(self) -> usize {
            match self {}
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Frame(u64);

    impl Clock for Frame {
        fn frames_since(self, start: Self) -> Option<u64> {
            self.0.checked_sub(start.0)
        }
    }

    impl Protocol for Test {
        type Applied = ();
        type Clock = Frame;
        type Command = Gain;
        type Refusal = ();
        type Target = NoTarget;
    }

    fn gain(level: f32) -> Gain {
        Gain::builder().level(level).build()
    }

    fn set(level: f32) -> GainUpdate {
        GainUpdate {
            level: GainLevelUpdate::Set { value: level },
        }
    }

    fn pair(capacity: usize) -> (Sender<Test>, Inbox<Test>) {
        let capacity = NonZeroUsize::new(capacity).expect("a test channel holds a batch");
        channel(ChannelConfig::builder().capacity(capacity).build())
    }

    /// Every batch due in the next block, as (offset, basis length, levels).
    fn render(inbox: &mut Inbox<Test>) -> Vec<(usize, usize, Vec<f32>)> {
        inbox.drain();
        let mut batches = Vec::new();
        while let Some(due) = inbox.next_due(Frame(0), 64) {
            let levels = due
                .commands()
                .iter()
                .map(|gain| gain.values().level)
                .collect();
            batches.push((due.offset(), due.basis().len(), levels));
            due.apply(());
        }
        batches
    }

    #[kithara::test]
    fn an_update_is_read_back_before_it_is_sent() {
        let (_sender, mut inbox) = pair(4);
        let mut live = Live::new(gain(0.5));

        live.update(set(0.25)).expect("an audible level");

        assert_eq!(
            live.config().values().level,
            0.25,
            "the owner reads its change at once"
        );
        assert!(
            render(&mut inbox).is_empty(),
            "nothing reaches the executor before a flush"
        );
    }

    #[kithara::test]
    fn a_flush_sends_the_latest_value_alone_for_the_next_block() {
        let (mut sender, mut inbox) = pair(4);
        let mut live = Live::new(gain(0.5));
        live.update(set(0.25)).expect("an audible level");
        live.update(set(0.75)).expect("an audible level");

        assert!(live.flush(&mut sender, core::convert::identity).is_some());
        assert_eq!(
            render(&mut inbox),
            vec![(0, 0, vec![0.75])],
            "one batch, at the block start, with an empty basis and the last value"
        );
        assert_eq!(
            live.flush(&mut sender, core::convert::identity),
            None,
            "a sent value is not sent again"
        );
    }

    #[kithara::test]
    fn a_full_channel_keeps_the_change_pending_until_a_credit_returns() {
        let (mut sender, mut inbox) = pair(1);
        let mut live = Live::new(gain(0.5));
        live.update(set(0.25)).expect("an audible level");
        assert!(live.flush(&mut sender, core::convert::identity).is_some());

        live.update(set(0.75)).expect("an audible level");
        assert_eq!(
            live.flush(&mut sender, core::convert::identity),
            None,
            "the only slot is in flight"
        );

        assert_eq!(render(&mut inbox), vec![(0, 0, vec![0.25])]);
        sender.receipts().for_each(drop);
        assert!(live.flush(&mut sender, core::convert::identity).is_some());
        assert_eq!(
            render(&mut inbox),
            vec![(0, 0, vec![0.75])],
            "the refused flush is retried once a credit returns"
        );
    }

    #[kithara::test]
    fn a_seed_takes_the_pending_change_and_sends_nothing() {
        let (mut sender, mut inbox) = pair(4);
        let mut live = Live::new(gain(0.5));
        live.update(set(0.25)).expect("an audible level");

        assert_eq!(
            live.seed().values().level,
            0.25,
            "a new executor starts from the change"
        );
        assert_eq!(
            live.flush(&mut sender, core::convert::identity),
            None,
            "the seeded executor already holds the value"
        );
        assert!(render(&mut inbox).is_empty());
    }

    #[kithara::test]
    fn a_refused_update_keeps_the_value_and_sends_nothing() {
        let (mut sender, mut inbox) = pair(4);
        let mut live = Live::new(gain(0.5));

        assert_eq!(live.update(set(0.0)), Err(Silence));
        assert_eq!(
            live.config().values().level,
            0.5,
            "a refused change leaves the value"
        );
        assert_eq!(live.flush(&mut sender, core::convert::identity), None);
        assert!(render(&mut inbox).is_empty());
    }
}
