use std::collections::HashSet;

use kithara_net::Net;

use crate::{Client, Error, MediaTrack, Playlist, PlaylistId, TrackId, TrackPage};

#[derive(Clone, PartialEq)]
pub(super) enum Mode {
    Search,
    Liked,
    Playlist(PlaylistId),
}

pub(super) struct Batch {
    pub(super) page: TrackPage,
    pub(super) streams: Result<Vec<MediaTrack>, Error>,
}

pub(super) enum Completed {
    Catalogue(u64, Option<Result<Batch, Error>>),
    Playlists(Option<Result<Vec<Playlist>, Error>>),
    Like(TrackId, bool, Option<Result<(), Error>>),
}

impl Completed {
    /// Whether the operation met the token rejection that latches the source.
    pub(super) fn rejects_authentication(&self) -> bool {
        matches!(
            self,
            Self::Catalogue(
                _,
                Some(
                    Err(Error::AuthenticationRejected)
                        | Ok(Batch {
                            streams: Err(Error::AuthenticationRejected),
                            ..
                        })
                )
            ) | Self::Playlists(Some(Err(Error::AuthenticationRejected)))
                | Self::Like(_, _, Some(Err(Error::AuthenticationRejected)))
        )
    }
}

impl Mode {
    pub(super) async fn load<N: Net>(
        &self,
        client: &Client<N>,
        query: &str,
    ) -> Result<Batch, Error> {
        let page = match self {
            Self::Search => client.search(query).await?,
            Self::Liked => client.liked_tracks().await?,
            Self::Playlist(id) => client.playlist_tracks(id).await?,
        };
        let mut listed = HashSet::new();
        let ids = page
            .tracks
            .iter()
            .filter(|track| listed.insert(&track.id))
            .map(|track| track.id.clone())
            .collect::<Vec<TrackId>>();
        let streams = client.streams(&ids).await;
        Ok(Batch { page, streams })
    }
}
