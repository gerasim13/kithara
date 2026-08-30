#[cfg(not(feature = "gui"))]
compile_error!("`kithara` binary requires the `gui` feature");

use std::num::NonZeroUsize;

use clap::Parser;
use kithara::{
    assets::{AssetStore, FlushHub, FlushPolicy, StorageBackend},
    bufpool::Region,
    host::{Host, HostConfig},
    net::{HttpClient, NetOptions},
    play::{PlayWorker, PlayWorkerConfig},
    stream::dl::{Downloader, DownloaderConfig},
};
use kithara_app::{
    baked,
    config::AppConfig,
    deck::{Deck, DeckId, DeckSet},
    gui::{self, GuiFrontend},
    tracing_init::init_tracing,
};
use kithara_platform::{CancelToken, thread, tokio};
use kithara_worker::{RayonConfig, Worker, WorkerConfig};

/// Kithara — audio player application.
#[derive(Parser)]
#[command(name = "kithara", about = "Audio player")]
struct Args {
    /// Audio files or URLs to play.
    tracks: Vec<String>,

    /// Accept invalid TLS certificates (self-signed, expired). For test servers only.
    /// Enabled by default during testing phase.
    #[arg(long, default_value_t = true)]
    insecure: bool,

    /// Which host draws the studio. A build without the `masonry` feature has
    /// only the immediate one.
    #[arg(long, value_enum, default_value_t)]
    host: gui::Host,

    /// Folder holding the UI package to draw from. Defaults to `assets/ui`
    /// beside the executable.
    #[arg(long)]
    ui_package: Option<std::path::PathBuf>,
}

/// Where a release lays its UI documents out: beside the executable.
fn shipped_ui_package() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("assets/ui"))
}

type AppError = Box<dyn std::error::Error + Send + Sync>;
type AppResult<T = ()> = Result<T, AppError>;

/// Suppress noisy macOS system logs (`OpenGL` `dlsym`, `WindowTab`, etc.)
/// at program start before any threads are spawned. No-op on other targets.
#[cfg(target_os = "macos")]
fn suppress_macos_system_logs() {
    // SAFETY: called at program start before any threads are spawned.
    unsafe {
        std::env::set_var("OS_ACTIVITY_MODE", "disable");
    }
}

#[cfg(not(target_os = "macos"))]
fn suppress_macos_system_logs() {}

fn main() -> AppResult {
    suppress_macos_system_logs();

    let args = Args::parse();
    init_tracing(&["info"])?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _runtime_guard = runtime.enter();

    // App master root held for the whole process: it goes into `AppConfig` and
    // every subsystem derives from `shutdown.child()`, so a frontend
    // `config.shutdown.cancel()` propagates through the whole app subtree.
    let shutdown = CancelToken::root();
    let region = Region::default();
    let byte_pool = region.byte_pool();
    let sample_pool = region.sample_pool();
    let compute_threads = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let base_worker = Worker::new(
        WorkerConfig::new()
            .with_cancel(shutdown.child())
            .with_runtime(runtime.handle().clone())
            .with_max_compute_tasks(compute_threads)
            .with_owned_pool(RayonConfig::new(compute_threads, "kithara-compute")),
    );
    let worker = PlayWorker::new(
        PlayWorkerConfig::for_pools(byte_pool.clone(), sample_pool)
            .cancel(shutdown.child())
            .worker(base_worker.clone())
            .build(),
    );
    let net = NetOptions::builder()
        .is_insecure(args.insecure || baked::BAKED_SHOULD_ACCEPT_INVALID_CERTS)
        .compression(baked::BAKED_COMPRESSION)
        .byte_pool(byte_pool.clone())
        .build();
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(net, shutdown.child())).build(),
    );
    let flush_hub = FlushHub::new(shutdown.child(), FlushPolicy::default());
    let store = AssetStore::builder()
        .cancel(shutdown.child())
        .backend(StorageBackend::default())
        .pool(byte_pool.clone())
        .flush_hub(flush_hub)
        .layouts(baked::build_baked_asset_layouts())
        .build();
    let config = AppConfig::builder()
        .downloader(downloader)
        .shutdown(shutdown.clone())
        .worker(worker)
        .base_worker(base_worker)
        .store(store)
        .maybe_tracks((!args.tracks.is_empty()).then_some(args.tracks))
        .should_accept_invalid_certs(args.insecure)
        .maybe_ui_package(args.ui_package.or_else(shipped_ui_package))
        .build();

    let mut host = Host::new(HostConfig::builder().build())?;
    let decks = vec![
        Deck::build(DeckId(0), &config, &mut host)?,
        Deck::build(DeckId(1), &config, &mut host)?,
    ];
    let mut deck_set = DeckSet::new(host, decks);
    deck_set.commit(deck_set.mix().clone())?;
    let mut frontend = GuiFrontend::new(&config, args.host)?;
    frontend.attach_broadcast(shutdown.clone());
    frontend.start(&deck_set)?;
    frontend.run_loop(deck_set)?;
    frontend.shutdown()?;

    Ok(())
}
