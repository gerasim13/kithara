#[cfg(not(feature = "gui"))]
compile_error!("`kithara` binary requires the `gui` feature");

use std::num::NonZeroUsize;

use clap::Parser;
use kithara::{
    assets::{FlushHub, FlushPolicy},
    host::HostConfig,
    net::{HttpClient, NetOptions},
    platform::{CancelToken, thread, tokio},
    play::PlayWorkerConfig,
    stream::dl::{Downloader, DownloaderConfig},
    worker::{RayonConfig, Worker, WorkerConfig},
};
use kithara_app::{
    config::{AppConfig, AppDrm},
    deck::{Deck, DeckId, DeckSet},
    document::Config,
    gui::{self, GuiFrontend},
    pools::{self, AppHost, AppStore, AppWorker},
    tracing_init::init_tracing,
};
use struct_patch::Patch as _;

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

    /// Configuration document to read. Defaults to `kithara.yaml` beside the
    /// executable when one is there.
    #[arg(long)]
    config: Option<std::path::PathBuf>,

    /// Print the effective configuration and exit.
    #[arg(long)]
    dump_config: bool,

    /// Audio files or URLs to play.
    tracks: Vec<String>,

    /// Accept invalid TLS certificates (self-signed, expired). For test servers only.
    /// An override on top of the document's `net.is_insecure`: `true` here
    /// forces it on regardless of the document.
    #[arg(long)]
    insecure: bool,
}

/// Where a release lays its UI documents out: beside the executable.
fn shipped_ui_package() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("assets/ui"))
}

/// Where an installation leaves its configuration: beside the executable.
fn config_beside_binary() -> Option<std::path::PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("kithara.yaml"))
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
    // Reported through `Display` and not returned: `main`'s error is printed
    // with `Debug`, which drops the readable list of unset `$KITHARA_...`
    // names. Tracing is not up yet either, so this goes to stderr directly.
    let document = match Config::load(args.config.as_deref(), config_beside_binary().as_deref()) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if args.dump_config {
        println!("{}", document.dump());
        return Ok(());
    }

    let settings = document.app_settings();
    let directives = settings
        .log_directives
        .clone()
        .unwrap_or_else(|| vec!["info".to_string()]);
    init_tracing(&directives.iter().map(String::as_str).collect::<Vec<&str>>())?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _runtime_guard = runtime.enter();

    let shutdown = CancelToken::root();
    let pools = pools::build(&document.pools())?;
    let compute_threads = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let mut worker_config = WorkerConfig::new()
        .with_cancel(shutdown.child())
        .with_runtime(runtime.handle().clone())
        .with_max_compute_tasks(compute_threads)
        .with_owned_pool(RayonConfig::new(compute_threads, "kithara-compute"));
    worker_config.apply(document.worker());
    if let Some(pool) = document.worker_pool() {
        worker_config = worker_config.with_pool_settings(pool);
    }
    let base_worker = Worker::new(worker_config);
    let worker = AppWorker::new(
        PlayWorkerConfig::builder(pools.clone())
            .cancel(shutdown.child())
            .worker(base_worker.clone())
            .build(),
    );
    let mut net = NetOptions::builder().build();
    net.apply(document.net());
    if args.insecure {
        net.is_insecure = true;
    }
    let should_accept_invalid_certs = net.is_insecure;
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(net, pools.clone(), shutdown.child())).build(),
    );
    let flush_hub = FlushHub::new(shutdown.child(), FlushPolicy::default());
    let store_settings = document.assets_store();
    let store = AppStore::builder(pools)
        .cancel(shutdown.child())
        .backend(document.store_backend())
        .flush_hub(flush_hub)
        .layouts(document.asset_layouts())
        .maybe_cache_capacity(store_settings.cache_capacity)
        .maybe_max_assets(store_settings.max_assets)
        .maybe_max_bytes(store_settings.max_bytes)
        .maybe_mem_resource_capacity(store_settings.mem_resource_capacity)
        .maybe_processing_chunk_size(store_settings.processing_chunk_size)
        .maybe_processing_gate_poll_interval(store_settings.processing_gate_poll_interval)
        .maybe_segment_reservation(store_settings.segment_reservation)
        .build();
    // `eprintln!`, not `tracing::error!`: tracing is up by now, but a startup
    // refusal must not depend on `RUST_LOG` to be seen.
    let drm_policy = match document.drm_policy() {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let mut config = AppConfig::builder()
        .drm(AppDrm::new(drm_policy))
        .downloader(downloader)
        .shutdown(shutdown.clone())
        .worker(worker)
        .base_worker(base_worker)
        .store(store)
        .size_probe_method(document.size_probe_method())
        // The same value tracing is running on, so a document that names no
        // directives still leaves the built configuration agreeing with the
        // process; one that names them has `apply` put back exactly this.
        .log_directives(directives)
        .maybe_crossfade_seconds(document.crossfade_seconds())
        .tracks(if args.tracks.is_empty() {
            document.tracks().to_vec()
        } else {
            args.tracks
        })
        .should_accept_invalid_certs(should_accept_invalid_certs)
        .maybe_ui_package(args.ui_package.or_else(shipped_ui_package))
        .build();
    config.apply(settings);

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
