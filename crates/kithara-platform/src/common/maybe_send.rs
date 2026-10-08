/// Wraps `T` with unconditional `Send` on WASM; native still requires `T: Send`.
/// Per-target implementations live in their platform backends.
///
/// # Safety contract
/// The value must never be actively used on multiple threads simultaneously.
/// Any `!Send` JS-backed values must be absent/uninitialized during transfer
/// and initialized only in the target Web Worker's local JS context.
/// `FileDownloader` meets this by moving with `writer: None` and creating its
/// HTTP stream through `ensure_writer()` inside the worker.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(get)]
pub struct WasmSend<T>(
    #[field(
        get(name = get, doc = "Immutable access."),
        get_mut(name = get_mut, doc = "Mutable access.")
    )]
    T,
);

impl<T> WasmSend<T> {
    /// Wrap a value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}
