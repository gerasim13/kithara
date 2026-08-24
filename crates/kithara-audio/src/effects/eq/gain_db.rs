use kithara_platform::ranged;

ranged!(
    /// Gain of one EQ band, in dB. `0.0` is unity and [`GainDb::MIN`] kills the
    /// band. The range is asymmetric on purpose: cutting stays useful far past
    /// the point where boosting only clips.
    pub struct GainDb(f32, -24.0, 6.0, 0.0)
);

impl GainDb {
    /// The gain a knob sitting at `knob` asks for: `0.0` is [`Self::MIN`],
    /// `0.5` is unity, `1.0` is [`Self::MAX`]. Each half of the travel is
    /// scaled against its own end, so unity stays in the middle of the knob
    /// even though the range around it is not symmetric.
    #[must_use]
    pub fn at_knob(knob: f32) -> Self {
        let offset = knob.clamp(0.0, 1.0) - 0.5;
        Self::from(2.0 * offset * Self::half_span(offset))
    }

    /// The knob position this gain sits at. Inverse of [`Self::at_knob`].
    #[must_use]
    pub fn knob(self) -> f32 {
        let db = f32::from(self);
        0.5 + db / (2.0 * Self::half_span(db))
    }

    fn half_span(side: f32) -> f32 {
        if side < 0.0 {
            -f32::from(Self::MIN)
        } else {
            f32::from(Self::MAX)
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn the_middle_of_the_knob_is_unity() {
        assert_eq!(GainDb::at_knob(0.5), GainDb::DEFAULT);
    }

    #[kithara::test]
    fn unity_sits_in_the_middle_of_the_knob() {
        assert_eq!(GainDb::default().knob(), 0.5);
    }

    #[kithara::test]
    fn the_bottom_of_the_knob_is_the_floor_of_the_range() {
        assert_eq!(GainDb::at_knob(0.0), GainDb::MIN);
    }

    #[kithara::test]
    fn the_top_of_the_knob_is_the_ceiling_of_the_range() {
        assert_eq!(GainDb::at_knob(1.0), GainDb::MAX);
    }

    /// The two halves are scaled separately, so the mapping is only usable if
    /// it survives a round trip at every position, not just at the three ends.
    #[kithara::test]
    fn a_knob_position_survives_the_round_trip() {
        for step in 0..=20u8 {
            let knob = f32::from(step) / 20.0;
            let round_trip = GainDb::at_knob(knob).knob();
            assert!(
                (round_trip - knob).abs() < 1e-6,
                "knob {knob} came back as {round_trip}"
            );
        }
    }

    #[kithara::test]
    fn a_knob_past_its_travel_is_held_at_the_end() {
        assert_eq!(GainDb::at_knob(2.0), GainDb::MAX);
        assert_eq!(GainDb::at_knob(-1.0), GainDb::MIN);
    }
}
