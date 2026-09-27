/// Frames one iteration of the strided copies moves. A loop that moves one
/// sample per iteration runs at half speed whenever it straddles a 4096-byte
/// page, and `opt-level = "z"` neither unrolls nor aligns it; four samples
/// per iteration amortize that fetch and roughly halve the cost everywhere.
pub(crate) const LANES: usize = 4;
