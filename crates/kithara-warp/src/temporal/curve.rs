/// Speed a renderer holds from the output frame a command applies on.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum SpeedCurve {
    /// One speed: media seconds consumed per output second.
    Constant(f32),
}
