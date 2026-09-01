#[cfg(not(feature = "gui"))]
compile_error!("`kithara` binary requires the `gui` feature");

use std::num::NonZeroUsize;

use clap::Parser;
use kithara::{
    assets::{FlushHub, FlushPolicy, StorageBackend},
    host::HostConfig,
    net::{HttpClient, NetOptions},
    platform::{CancelToken, thread, tokio},
    play::PlayWorkerConfig,
    stream::dl::{Downloader, DownloaderConfig},
    worker::{RayonConfig, Worker, WorkerConfig},
};
use kithara_app::{
    baked,
    config::AppConfig,
    deck::{Deck, DeckId, DeckSet},
    gui::{self, GuiFrontend},
    pools::{self, AppHost, AppStore, AppWorker},
    tracing_init::init_tracing,
};

/// Kithara — audio player application.
#[derive(Parser)]
#[command(name = "kithara", about = "Audio player")]
struct Args {
    /// Which host draws the studio. A build without the `masonry` feature has
    /// only the immediate one.
    #[arg(long, value_enum, default_value_t)]
    host: gui::Host,

    /// Folder holding the UI package to draw from. Defaults to `assets/ui`
    /// beside the executable.
    #[arg(long)]
    ui_package: Option<std::path::PathBuf>,

    /// Audio files or URLs to play.
    tracks: Vec<String>,

    /// Accept invalid TLS certificates (self-signed, expired). For test servers only.
    /// Enabled by default during testing phase.
    #[arg(long, default_value_t = true)]
    insecure: bool,
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

    let shutdown = CancelToken::root();
    let pools = pools::build()?;
    let compute_threads = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let base_worker = Worker::new(
        WorkerConfig::new()
            .with_cancel(shutdown.child())
            .with_runtime(runtime.handle().clone())
            .with_max_compute_tasks(compute_threads)
            .with_owned_pool(RayonConfig::new(compute_threads, "kithara-compute")),
    );
    let worker = AppWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(shutdown.child())
            .worker(base_worker.clone())
            .build(),
    );
    let net = NetOptions::builder()
        .is_insecure(args.insecure || baked::BAKED_SHOULD_ACCEPT_INVALID_CERTS)
        .compression(baked::BAKED_COMPRESSION)
        .build();
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(net, pools.clone(), shutdown.child())).build(),
    );
    let flush_hub = FlushHub::new(shutdown.child(), FlushPolicy::default());
    let store = AppStore::builder(pools)
        .cancel(shutdown.child())
        .backend(StorageBackend::default())
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

    let mut host = AppHost::new(HostConfig::builder().build())?;
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
