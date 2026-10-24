use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, stream};
use http::{Request, Response};
use kithara_net::{
    HttpBody, HttpClient, HttpTransport, NetError, NetObserver, NetOptions, Observer, RetryPolicy,
};
use kithara_platform::{
    CancelToken,
    sync::Arc,
    time::{Duration, Instant, WallInstant, sleep},
};
use kithara_test_utils::kithara;
use url::Url;

struct Supplied {
    requests: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Arrivals(AtomicUsize);

impl NetObserver for Arrivals {
    fn first_byte(&self, _ttfb: Duration, status: u16, partial: bool) {
        assert_eq!(status, 200);
        assert!(!partial);
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl HttpTransport for Supplied {
    async fn open(&self, request: Request<Bytes>) -> Result<Response<HttpBody>, NetError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let mut response = Response::builder();
        let body: HttpBody = match request.uri().path() {
            "/head" => {
                sleep(Duration::from_secs(2)).await;
                Box::pin(stream::iter([Ok(Bytes::from_static(b"head"))]))
            }
            "/body" => Box::pin(stream::iter([b"one", b"two", b"end"]).then(|bytes| async {
                sleep(Duration::from_millis(750)).await;
                Ok(Bytes::from_static(bytes))
            })),
            "/error-empty" => {
                response = response.status(404);
                Box::pin(stream::empty())
            }
            "/error-long" => {
                response = response.status(404);
                Box::pin(stream::iter([Ok(Bytes::from("\u{0416}".repeat(201)))]))
            }
            "/error-stall" => {
                response = response.status(404);
                Box::pin(stream::pending())
            }
            _ => return Err(NetError::Refused("unknown supplied route".to_owned())),
        };
        Ok(response.body(body).unwrap())
    }
}

fn client(inactivity: Duration) -> (HttpClient, Arc<AtomicUsize>) {
    let requests = Arc::new(AtomicUsize::new(0));
    let client = HttpClient::new_with_transport(
        NetOptions::builder()
            .inactivity_timeout(inactivity)
            .retry_policy(RetryPolicy::builder().max_retries(0).build())
            .build(),
        Supplied {
            requests: Arc::clone(&requests),
        },
        CancelToken::never(),
    );
    (client, requests)
}

fn url(path: &str) -> Url {
    Url::parse(&format!("http://fixture.invalid{path}")).unwrap()
}

#[kithara::test(tokio, flash(true))]
#[case::headers("/head", Duration::from_secs(2), Duration::from_secs(3), b"head")]
#[case::fragments(
    "/head#ignored",
    Duration::from_secs(2),
    Duration::from_secs(3),
    b"head"
)]
#[case::chunks(
    "/body",
    Duration::from_millis(2250),
    Duration::from_secs(1),
    b"onetwoend"
)]
async fn modeled_latency_spends_virtual_time(
    #[case] path: &str,
    #[case] delay: Duration,
    #[case] inactivity: Duration,
    #[case] expected: &[u8],
) {
    let (client, requests) = client(inactivity);
    let wall = WallInstant::now();
    let start = Instant::now();
    assert_eq!(
        client.get_bytes(url(path), None).await.unwrap().as_ref(),
        expected
    );
    assert!(start.elapsed() >= delay);
    assert!(wall.elapsed() < Duration::from_secs(1));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[kithara::test(tokio, flash(true))]
#[case::headers("/head")]
#[case::chunks("/body")]
async fn modeled_latency_can_exceed_inactivity(#[case] path: &str) {
    let inactivity = Duration::from_millis(500);
    let (client, requests) = client(inactivity);
    let start = Instant::now();
    let wall = WallInstant::now();
    assert!(matches!(
        client.get_bytes(url(path), None).await,
        Err(NetError::Timeout)
    ));
    assert!(start.elapsed() >= inactivity);
    assert!(wall.elapsed() < Duration::from_secs(1));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[kithara::test(tokio, flash(true))]
#[case::empty("/error-empty", String::new())]
#[case::long("/error-long", format!("{}\u{2026}(truncated, 201 chars total)", "\u{0416}".repeat(200)))]
#[case::stalled("/error-stall", String::new())]
async fn refused_body_has_one_diagnostic_contract(#[case] path: &str, #[case] expected: String) {
    let (client, _) = client(Duration::from_millis(500));
    let error = client.get_bytes(url(path), None).await.unwrap_err();
    assert!(
        matches!(error, NetError::Status { status, body: Some(body), .. }
        if status.get() == 404 && body == expected)
    );
}

#[kithara::test(tokio, flash(true))]
async fn derived_handles_retain_the_supplied_transport() {
    let (client, requests) = client(Duration::from_secs(3));
    let arrivals = Arc::new(Arrivals::default());
    let observed = client.with_observer(Some(Observer(arrivals.clone())));
    let derived = observed.with_retry_policy(RetryPolicy::builder().max_retries(2).build());
    for (handle, expected) in [(&client, 0), (&observed, 1), (&derived, 2)] {
        assert_eq!(handle.get_bytes(url("/head"), None).await.unwrap(), "head");
        assert_eq!(arrivals.0.load(Ordering::SeqCst), expected);
    }
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    assert_eq!(
        derived
            .with_observer(None)
            .get_bytes(url("/head"), None)
            .await
            .unwrap(),
        "head"
    );
    assert_eq!(arrivals.0.load(Ordering::SeqCst), 2);
    assert!(matches!(
        derived.get_bytes(url("/unknown"), None).await,
        Err(NetError::Refused(_))
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 5);
}

#[kithara::test(tokio)]
#[case::get_headers("GET", "/head")]
#[case::post_headers("POST", "/head")]
#[case::head_headers("HEAD", "/head")]
#[case::get_body("GET", "/body")]
#[case::post_body("POST", "/body")]
#[case::head_body("HEAD", "/body")]
#[case::get_diagnostic("GET", "/error-stall")]
#[case::post_diagnostic("POST", "/error-stall")]
#[case::head_diagnostic("HEAD", "/error-stall")]
async fn cancellation_interrupts_active_delivery(#[case] method: &str, #[case] path: &str) {
    let requests = Arc::new(AtomicUsize::new(0));
    let cancel = kithara_test_utils::cancel_token();
    let client = HttpClient::new_with_transport(
        NetOptions::default(),
        Supplied {
            requests: Arc::clone(&requests),
        },
        cancel.clone(),
    );
    let derived = client
        .with_observer(None)
        .with_retry_policy(RetryPolicy::default());
    let mut request = Box::pin(async {
        match method {
            "GET" => derived.get_bytes(url(path), None).await,
            "POST" => derived.post_bytes(url(path), Bytes::new(), None).await,
            "HEAD" => derived.head(url(path), None).await.map(|_| Bytes::new()),
            _ => unreachable!(),
        }
    });
    assert!(futures::poll!(request.as_mut()).is_pending());
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    cancel.cancel();
    assert!(matches!(
        futures::poll!(request.as_mut()),
        std::task::Poll::Ready(Err(NetError::Cancelled))
    ));
    assert!(matches!(
        client.get_bytes(url(path), None).await,
        Err(NetError::Cancelled)
    ));
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "no cancelled request or retry opens"
    );
}
