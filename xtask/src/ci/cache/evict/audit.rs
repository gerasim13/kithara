use std::{
    io::Read,
    sync::mpsc::{SyncSender, TrySendError},
};

use serde::Deserialize;
use tiny_http::{Response, Server};
use tracing::debug;

use super::entry::Entry;
use crate::consts;

/// What one audited request says about a scope's bucket, which `scope`
/// names by its place in the evictor's list.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// A client read or wrote an entry, at this second of the store's clock.
    Use {
        scope: usize,
        entry: Entry,
        at_s: u64,
    },
    /// The evictor's own marker was asked for, so the log is arriving.
    Echo { scope: usize },
    /// Any other request: a miss, a snapshot, a delete.
    Activity { scope: usize },
}

/// The events of one request, and how many events the receiver could not
/// hand over since its last delivery because the owner had fallen behind.
#[derive(Debug)]
pub(super) struct Delivery {
    pub(super) events: Vec<Event>,
    pub(super) dropped: u64,
}

/// What the store's webhook posts: one audit entry per request, wrapped.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TargetLog {
    records: Vec<Record>,
}

#[derive(Deserialize)]
struct Record {
    /// Milliseconds since the epoch on the store's clock.
    time: u64,
    api: Api,
}

#[derive(Deserialize)]
struct Api {
    name: Option<String>,
    bucket: Option<String>,
    object: Option<String>,
    status_code: Option<i32>,
}

/// The events a delivery carries about the buckets in `managed`. A body that
/// is not an audit log carries none.
pub(super) fn events(body: &[u8], managed: &[String]) -> Vec<Event> {
    let Ok(log) = serde_json::from_slice::<TargetLog>(body) else {
        return Vec::new();
    };
    log.records
        .into_iter()
        .filter_map(|record| {
            let api = record.api;
            let scope = managed
                .iter()
                .position(|bucket| api.bucket.as_ref() == Some(bucket))?;
            let object = api.object.as_deref();
            let used = matches!(api.name.as_deref(), Some("s3:GetObject" | "s3:PutObject"))
                && api
                    .status_code
                    .is_some_and(|code| (200..300).contains(&code));
            Some(if object == Some(consts::EVICT_MARKER) {
                Event::Echo { scope }
            } else if let Some(entry) = object.filter(|_| used).and_then(Entry::parse) {
                Event::Use {
                    scope,
                    entry,
                    at_s: record.time / 1000,
                }
            } else {
                Event::Activity { scope }
            })
        })
        .collect()
}

/// Hands each request's events to the owner without ever waiting on it, then
/// answers. The store sends from a task of its own and keeps what it could not
/// deliver in its queue; a receiver that blocked would only move that queue
/// here. By the time the store has its answer, the events are with the owner
/// or counted as dropped.
pub(super) fn receive(server: &Server, managed: &[String], sender: &SyncSender<Delivery>) {
    let mut dropped = 0;
    for mut request in server.incoming_requests() {
        let mut body = Vec::new();
        let read = request
            .as_reader()
            .take(consts::AUDIT_BODY_LIMIT)
            .read_to_end(&mut body);
        let events = read.map_or_else(|_| Vec::new(), |_| events(&body, managed));
        if !events.is_empty() {
            let count = events.len() as u64;
            match sender.try_send(Delivery { events, dropped }) {
                Ok(()) => dropped = 0,
                Err(TrySendError::Full(_)) => dropped += count,
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        if let Err(error) = request.respond(Response::empty(200)) {
            debug!(%error, "the store left before its delivery was answered");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread};

    use super::*;

    fn managed() -> Vec<String> {
        vec!["kithara-trusted".to_owned(), "kithara-review".to_owned()]
    }

    fn delivery(name: &str, bucket: &str, object: &str, status_code: i32) -> String {
        serde_json::json!({
            "EventName": "s3:ObjectAccessed:Get",
            "Key": format!("{bucket}/{object}"),
            "Records": [{
                "version": "1",
                "time": 1_759_658_400_123_u64,
                "event": "s3:ObjectAccessed:Get",
                "trigger": "incoming",
                "api": {
                    "name": name,
                    "bucket": bucket,
                    "object": object,
                    "status": "OK",
                    "status_code": status_code,
                },
                "userAgent": "opendal",
            }],
        })
        .to_string()
    }

    fn entry_object() -> String {
        format!("sccache/a/b/c/{}", consts::ENTRY_HASH)
    }

    #[test]
    fn a_successful_read_or_write_of_an_entry_is_a_use() {
        let entry = Entry::parse(&entry_object()).unwrap();
        for (name, status) in [
            ("s3:GetObject", 200),
            ("s3:GetObject", 206),
            ("s3:PutObject", 200),
        ] {
            let body = delivery(name, "kithara-review", &entry_object(), status);

            assert_eq!(
                events(body.as_bytes(), &managed()),
                [Event::Use {
                    scope: 1,
                    entry,
                    at_s: 1_759_658_400,
                }],
                "{name}"
            );
        }
    }

    /// A miss is the request a compile makes before it writes the entry, so
    /// it says a client is busy and nothing about the entry's last use.
    #[test]
    fn a_miss_or_a_snapshot_is_activity_not_a_use() {
        for body in [
            delivery("s3:GetObject", "kithara-trusted", &entry_object(), 404),
            delivery(
                "s3:GetObject",
                "kithara-trusted",
                "target-snapshots/a/b.tar",
                200,
            ),
            delivery("s3:DeleteObject", "kithara-trusted", &entry_object(), 204),
        ] {
            assert_eq!(
                events(body.as_bytes(), &managed()),
                [Event::Activity { scope: 0 }],
                "{body}"
            );
        }
    }

    #[test]
    fn a_request_for_the_marker_echoes() {
        let body = delivery("s3:HeadObject", "kithara-review", consts::EVICT_MARKER, 404);

        assert_eq!(
            events(body.as_bytes(), &managed()),
            [Event::Echo { scope: 1 }]
        );
    }

    /// The store reports every bucket it serves, the evictor's own record
    /// among them; only the scopes this stack provisions are its to evict.
    #[test]
    fn a_bucket_outside_the_scopes_says_nothing() {
        let body = delivery("s3:PutObject", consts::RECENCY_BUCKET, &entry_object(), 200);

        assert!(events(body.as_bytes(), &managed()).is_empty());
        assert!(events(b"not an audit log", &managed()).is_empty());
        assert!(events(br#"{"Records":[{"time":1,"api":{}}]}"#, &managed()).is_empty());
    }

    #[test]
    fn the_receiver_answers_at_once_and_reports_what_a_full_owner_dropped() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let (sender, deliveries) = mpsc::sync_channel(1);
        thread::spawn(move || receive(&server, &managed(), &sender));
        let client = reqwest::blocking::Client::new();
        let post = |body: String| {
            let response = client
                .post(format!("http://{address}/"))
                .body(body)
                .send()
                .unwrap();
            assert!(response.status().is_success(), "{}", response.status());
        };
        let body = delivery("s3:GetObject", "kithara-review", &entry_object(), 200);

        for _ in 0..3 {
            post(body.clone());
        }
        let first = deliveries.recv().unwrap();
        post(body);
        let next = deliveries.recv().unwrap();

        assert_eq!((first.events.len(), first.dropped), (1, 0));
        assert_eq!((next.events.len(), next.dropped), (1, 2));
    }
}
