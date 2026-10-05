#![forbid(unsafe_code)]

mod client;
mod config;
mod consts;
mod error;
mod media;
mod model;
mod page;
mod ui;

pub use crate::{client::Client, config::Config, ui::Source};
pub(crate) use crate::{
    error::{Error, GraphQlError},
    media::MediaTrack,
    model::{Playlist, PlaylistId, Track, TrackId},
    page::TrackPage,
};
