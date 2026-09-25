#![cfg(feature = "render")]

//! The two hosts laid out, drawn and driven side by side, through the public
//! API.

#[cfg(all(feature = "masonry", feature = "iced"))]
mod census;
#[path = "../common/mod.rs"]
mod common;
#[cfg(all(feature = "masonry", feature = "iced"))]
mod immediate;
mod layout;
#[cfg(all(feature = "masonry", feature = "iced"))]
mod shared;
#[cfg(all(feature = "masonry", feature = "iced"))]
mod used;
