use std::path::PathBuf;

use kithara::platform::tokio::{runtime::Handle, sync::mpsc::UnboundedSender, task};
use rfd::AsyncFileDialog;
use tracing::debug;

use super::explorer::Found;

/// The folders added to Music Folders, in the order added.
#[derive(Default, fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(super) struct MusicFolders {
    #[field(get = list, vis = "pub(super)")]
    folders: Vec<PathBuf>,
}

impl MusicFolders {
    pub(super) fn add(&mut self, folder: PathBuf) {
        if !self.folders.contains(&folder) {
            self.folders.push(folder);
        }
    }
}

/// Asks for a folder to add; Explorer takes the answer on a later tick.
#[derive(Clone)]
pub(in crate::gui) struct FolderPicker {
    found: UnboundedSender<Found>,
    runtime: Handle,
}

impl FolderPicker {
    pub(super) const fn new(found: UnboundedSender<Found>, runtime: Handle) -> Self {
        Self { found, runtime }
    }

    pub(in crate::gui) fn open(&self) {
        let picker = self.clone();
        drop(task::spawn_on(&self.runtime, async move {
            picker.picked(
                AsyncFileDialog::new()
                    .pick_folder()
                    .await
                    .map(|folder| folder.path().to_path_buf()),
            );
        }));
    }

    pub(in crate::gui::library) fn picked(&self, folder: Option<PathBuf>) {
        let Some(folder) = folder else {
            return;
        };
        if self.found.send(Found::Picked(folder)).is_err() {
            debug!("the library closed before the picked folder arrived");
        }
    }
}
