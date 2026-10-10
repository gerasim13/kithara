use bytes::Bytes;
use kithara_app_library::AccessToken;
use kithara_net::Net;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use url::Url;

use crate::{Client, consts};

/// One answer while a device code waits.
#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub(super) enum Poll {
    Pending {},
    Success { access_token: String },
}

/// A device code the user confirms in the browser.
#[derive(Deserialize)]
pub(super) struct Session {
    pub(super) device_code: String,
    pub(super) expires_in: u64,
    #[serde(rename = "qr_authorization_url")]
    pub(super) url: Url,
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct ProfileReply {
    profile: Profile,
}

#[derive(Deserialize)]
struct Profile {
    name: Option<String>,
}

/// Zvuk's account requests.
#[derive(Clone)]
pub(super) struct Auth<N> {
    client: Client<N>,
    session: Url,
    token: Url,
    profile: Url,
    logout: Url,
}

impl<N: Net> Auth<N> {
    pub(super) fn new(client: Client<N>) -> Self {
        Self {
            client,
            session: endpoint(consts::SESSION_ENDPOINT),
            token: endpoint(consts::TOKEN_ENDPOINT),
            profile: endpoint(consts::PROFILE_ENDPOINT),
            logout: endpoint(consts::LOGOUT_ENDPOINT),
        }
    }

    /// Starts a device authorization for a fresh device id.
    pub(super) async fn session(&self) -> Option<Session> {
        let device_id = uuid::Builder::from_random_bytes(rand::random()).into_uuid();
        let body = json!({ "device_id": device_id.to_string() }).to_string();
        let mut headers = self.client.headers(None);
        headers.insert("Content-Type", "application/json");
        decode(
            self.client
                .mutations
                .post_bytes(self.session.clone(), Bytes::from(body), Some(headers))
                .await,
        )
    }

    /// The code's state; `None` when the reply does not read.
    pub(super) async fn poll(&self, device_code: &str) -> Option<Poll> {
        let mut url = self.token.clone();
        url.query_pairs_mut()
            .append_pair("device_code", device_code);
        decode(
            self.client
                .net
                .get_bytes(url, Some(self.client.headers(None)))
                .await,
        )
    }

    /// The profile name; empty or unknown reads `None`.
    pub(super) async fn label(&self, token: &AccessToken) -> Option<String> {
        let reply: ProfileReply = decode(
            self.client
                .net
                .get_bytes(self.profile.clone(), Some(self.client.headers(Some(token))))
                .await,
        )?;
        reply.profile.name.filter(|name| !name.is_empty())
    }

    /// Asks Zvuk to revoke the session `token` belongs to.
    pub(super) async fn logout(&self, token: &AccessToken) {
        let _ = self
            .client
            .mutations
            .post_bytes(
                self.logout.clone(),
                Bytes::new(),
                Some(self.client.headers(Some(token))),
            )
            .await;
    }
}

fn endpoint(url: &str) -> Url {
    Url::parse(url).expect("BUG: the endpoint is a valid URL")
}

fn decode<T: DeserializeOwned, E>(reply: Result<Bytes, E>) -> Option<T> {
    let body = reply.ok()?;
    serde_json::from_slice::<Envelope<T>>(&body)
        .ok()
        .map(|envelope| envelope.result)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use bytes::Bytes;
    use kithara_app_library::AccessToken;
    use kithara_net::mock::NetMock;
    use kithara_test_utils::kithara;
    use serde_json::json;
    use unimock::{MockFn, Unimock, matching};

    use super::Auth;
    use crate::{Client, Config};

    /// Starting a sign-in and logging out change sessions at Zvuk, so each is
    /// sent once, through the mutation transport.
    #[kithara::test]
    async fn a_session_start_and_a_logout_go_through_the_mutation_transport() {
        let session = json!({"result": {
            "device_code": "synthetic-device-code",
            "expires_in": 300,
            "qr_authorization_url": "https://id.example.com/",
        }});
        let mutations = Unimock::new((
            NetMock::post_bytes
                .next_call(matching!((url, _, _) if url.path() == "/api/tiny/login/qr/session"))
                .returns(Ok(Bytes::from(session.to_string()))),
            NetMock::post_bytes
                .next_call(matching!((url, _, _) if url.path() == "/api/tiny/logout"))
                .returns(Ok(Bytes::from_static(br#"{"result": null}"#))),
        ));
        let config = Config {
            user_agent: "synthetic-agent".to_owned(),
        };
        let client = Client::with_transports(Unimock::new(()), mutations, &config);
        let auth = Auth::new(client);

        assert!(auth.session().await.is_some());
        auth.logout(&AccessToken::new("synthetic-token".to_owned()))
            .await;
    }
}
