#[cfg(feature = "iced")]
mod clipboard;
#[cfg(feature = "iced")]
mod paint;
#[cfg(feature = "iced")]
mod program;
#[cfg(feature = "iced")]
mod widget;

#[cfg(all(test, feature = "iced"))]
pub(crate) use clipboard::ClipboardText;
#[cfg(feature = "iced")]
pub(crate) use clipboard::paste;
#[cfg(feature = "iced")]
pub(crate) use widget::{search_input, sync_text_input};
