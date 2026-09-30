use std::ops::Deref;

use kithara_ui::render::{ControlAction, Published};

use crate::{Gallery, Message, Page, Shot, sections, update};

pub(crate) struct Hand(Gallery);

impl Hand {
    pub(crate) fn at(tab: Page) -> Self {
        Self::standing(Shot { tab, module: None })
    }

    pub(crate) fn at_module(module: Page) -> Self {
        Self::standing(Shot {
            tab: sections::MODULES,
            module: Some(module),
        })
    }

    fn standing(shot: Shot) -> Self {
        let mut gallery = Gallery::mounted();
        gallery.select(shot);
        Self(gallery)
    }

    pub(crate) fn gesture(&mut self, path: &str, action: ControlAction) {
        let published = Published::Gesture {
            action,
            path: path.to_owned(),
        };
        drop(update(&mut self.0, Message::Ui(published)));
    }

    pub(crate) fn press(&mut self, path: &str) {
        self.gesture(path, ControlAction::Activate);
    }
}

impl Deref for Hand {
    type Target = Gallery;

    fn deref(&self) -> &Gallery {
        &self.0
    }
}
