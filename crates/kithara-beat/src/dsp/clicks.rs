pub(crate) fn positions(seconds: f32, period_seconds: f32) -> Vec<f32> {
    let mut at = 0.0;
    let mut out = Vec::new();
    while at < seconds {
        out.push(at);
        at += period_seconds;
    }
    out
}
