use std::collections::HashSet;

use kithara_app_library::AccessToken;
use kithara_net::Net;

use crate::{Client, Error, MediaTrack, PlaylistId, TrackId, TrackPage};

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

impl Mode {
    /// The node's page and its streams; a refused token fails the whole batch.
    pub(super) async fn load<N: Net>(
        &self,
        client: &Client<N>,
        token: &AccessToken,
        query: &str,
    ) -> Result<Batch, Error> {
        let page = match self {
            Self::Search => client.search(token, query).await?,
            Self::Liked => client.liked_tracks(token).await?,
            Self::Playlist(id) => client.playlist_tracks(token, id).await?,
        };
        let mut listed = HashSet::new();
        let ids = page
            .tracks
            .iter()
            .filter(|track| listed.insert(&track.id))
            .map(|track| track.id.clone())
            .collect::<Vec<TrackId>>();
        let streams = match client.streams(token, &ids).await {
            Err(Error::AuthenticationRejected) => return Err(Error::AuthenticationRejected),
            streams => streams,
        };
        Ok(Batch { page, streams })
    }
}
