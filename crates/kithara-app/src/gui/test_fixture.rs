use std::{cell::RefCell, path::Path, rc::Rc};

use arc_swap::ArcSwap;
use kithara::{
    assets::StorageBackend,
    download::{Downloader, DownloaderConfig},
    net::{HttpClient, NetOptions},
    platform::{CancelToken, sync::Arc, tokio::sync::mpsc::UnboundedSender},
    play::{PlayWorkerConfig, policy::DomainKeyPolicy},
    ui::{error::UiDocError, module::IconName, render::TableRow, text::TextDoc},
};

use super::{
    frontend::Boot,
    library::{
        BranchNode, Library, LibrarySource, PageStatus, PagesModule, Registration, SourcePage,
        StartupSource, worded,
    },
    ui::package::Package,
};
use crate::{
    config::{AppConfig, AppDrm},
    engine::{EngineSnapshot, Envelope},
    pools::{self, AppStore, AppWorker, PoolsSection},
};

pub(super) fn config() -> AppConfig {
    let shutdown = CancelToken::root();
    let pools = pools::build(&PoolsSection::default()).expect("valid app pool policy");
    let worker = AppWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(
            NetOptions::builder().build(),
            pools.clone(),
            shutdown.child(),
        ))
        .build(),
    );
    let store = AppStore::builder(pools)
        .backend(StorageBackend::Memory)
        .build();
    AppConfig::builder()
        .drm(AppDrm::new(DomainKeyPolicy::new(Vec::new())))
        .downloader(downloader)
        .shutdown(shutdown)
        .worker(worker)
        .store(store)
        .build()
}

pub(super) fn runtime() -> kithara::platform::tokio::runtime::Runtime {
    kithara::platform::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime builds")
}

pub(super) fn boot(
    runtime: &kithara::platform::tokio::runtime::Handle,
    config: &AppConfig,
    snapshots: Arc<ArcSwap<EngineSnapshot>>,
    commands: UnboundedSender<Envelope>,
) -> Boot {
    Boot::builder()
        .settings(&config.ui)
        .tracks(vec![
            "/music/local.flac".to_string(),
            "https://example.test/stream.m3u8".to_string(),
        ])
        .palette(config.palette)
        .snapshots(snapshots)
        .commands(commands)
        .runtime(runtime.clone())
        .build()
        .expect("shipped UI compiles")
}

pub(super) fn mount(
    root: Option<&Path>,
    registered: Vec<Registration>,
) -> Result<(Rc<Package>, Library), UiDocError> {
    let package = Package::load(root, PagesModule::new(&registered).into())?;
    let library = Library::new(registered, package.text())?;
    Ok((package, library))
}

pub(super) fn package(root: Option<&Path>) -> Result<Rc<Package>, UiDocError> {
    Package::load(
        root,
        PagesModule::new(&[StartupSource::registered(Vec::new())]).into(),
    )
}

/// A source only a test registers: a folder holding one more, and a leaf.
pub(super) struct Probe {
    branch: BranchNode,
    calls: Rc<RefCell<Calls>>,
}

/// The nodes the library has told a [`Probe`] about so far, in order.
#[derive(Default)]
pub(super) struct Calls {
    pub(super) expanded: Vec<String>,
    pub(super) selected: Vec<String>,
    /// How many times its rows were built.
    pub(super) rows: usize,
}

impl Probe {
    pub(super) const ID: &'static str = "probe";

    pub(super) fn registered(label: &'static str) -> (Registration, Rc<RefCell<Calls>>) {
        let calls = Rc::new(RefCell::new(Calls::default()));
        let told = Rc::clone(&calls);
        let registration = Registration::new(SourcePage::table(Self::ID), move |text| {
            Ok(Box::new(Self::new(label, text, told)?))
        });
        (registration, calls)
    }

    fn new(label: &str, text: &TextDoc, calls: Rc<RefCell<Calls>>) -> Result<Self, UiDocError> {
        let node = |key: &str, children| BranchNode {
            children,
            ..BranchNode::new(key, key.to_owned(), IconName::Folder)
        };
        let mut branch = node(
            Self::ID,
            vec![
                node("crate", vec![node("digger", Vec::new())]),
                node("leaf", Vec::new()),
            ],
        );
        branch.label = worded(text, label, Self::ID)?;
        Ok(Self { branch, calls })
    }
}

impl LibrarySource for Probe {
    fn analysis_key(&self, _row: usize) -> Option<&str> {
        None
    }

    fn branch(&self) -> &BranchNode {
        &self.branch
    }

    fn expand(&mut self, node: &str) {
        self.calls.borrow_mut().expanded.push(node.to_owned());
    }

    fn id(&self) -> &str {
        Self::ID
    }

    fn row_key(&self, _row: usize) -> Option<&str> {
        None
    }

    fn rows(&self, _selected: Option<&str>) -> Vec<TableRow<'_>> {
        self.calls.borrow_mut().rows += 1;
        Vec::new()
    }

    fn select(&mut self, node: &str) {
        self.calls.borrow_mut().selected.push(node.to_owned());
    }

    fn status(&self) -> PageStatus {
        PageStatus::Empty
    }

    fn tick(&mut self) {}
}
