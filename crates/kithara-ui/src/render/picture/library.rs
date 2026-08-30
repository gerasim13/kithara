use std::collections::BTreeMap;

use kithara_platform::sync::Arc;

use super::sprite::Sheet;
use crate::{error::UiDocError, skin::PictureDoc, source::SourceResolver};

/// Every picture one skin carries, cut into frames and kept by the name a
/// document asks for it by.
///
/// Cutting happens once, while the skin resolves, because a frame is its own
/// picture with its own identity: a rasteriser uploads each one once and every
/// later frame of the animation is a lookup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Pictures {
    sheets: BTreeMap<String, Arc<Sheet>>,
}

impl Pictures {
    /// Reads every picture the skin names and cuts it on the grid it declares.
    ///
    /// A picture the skin names and the resolver cannot answer is an error
    /// rather than an empty slot: the skin declared it, so a build without it
    /// is a broken skin, not a skin with one drawing fewer.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when a picture is missing, escapes the root, or
    /// does not cut into the frames the skin declares.
    pub(crate) fn load(
        document: &PictureDoc,
        resolver: &dyn SourceResolver,
    ) -> Result<Self, UiDocError> {
        let mut sheets = BTreeMap::new();
        for (name, declared) in &document.sheets {
            let loaded = resolver.bytes(None, &declared.source)?;
            let cut = Sheet::cut(name, &loaded.bytes, declared.columns, declared.rows).map_err(
                |source| UiDocError::Picture {
                    name: name.clone(),
                    origin: loaded.uri,
                    source: Box::new(source),
                },
            )?;
            sheets.insert(name.clone(), Arc::new(cut));
        }
        Ok(Self { sheets })
    }

    /// The picture one name means, or nothing when this skin carries none by
    /// that name.
    #[must_use]
    pub fn sheet(&self, name: &str) -> Option<&Arc<Sheet>> {
        self.sheets.get(name)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::Pictures;
    use crate::{
        builtin,
        draw::Image,
        ids::SourceUri,
        render::Skin,
        skin::{PictureDoc, SheetDoc},
    };

    fn doc(source: &str, columns: u32) -> PictureDoc {
        let mut document = PictureDoc::default();
        document.sheets.insert(
            "spinner".to_owned(),
            SheetDoc {
                columns,
                rows: 1,
                source: source.to_owned(),
            },
        );
        document
    }

    #[kithara::test]
    fn the_skin_answers_the_name_it_declared() {
        let pictures = Pictures::load(&doc("sprites/spinner.png", 8), &builtin::resolver())
            .unwrap_or_else(|error| panic!("a declared picture must load: {error}"));

        assert_eq!(pictures.sheet("spinner").map(|sheet| sheet.len()), Some(8));
    }

    /// A document naming a picture the skin does not carry draws nothing,
    /// rather than standing in for it with one it did not ask for.
    #[kithara::test]
    fn a_name_the_skin_carries_nothing_for_answers_nothing() {
        let pictures = Pictures::load(&doc("sprites/spinner.png", 8), &builtin::resolver())
            .unwrap_or_else(|error| panic!("a declared picture must load: {error}"));

        assert!(pictures.sheet("no-such-picture").is_none());
    }

    /// The picture is cut once and shared: a frame drawn on one screen and the
    /// same frame drawn on the next are one picture to whatever uploads it.
    #[kithara::test]
    fn asking_twice_gives_back_the_same_cut() {
        let pictures = Pictures::load(&doc("sprites/spinner.png", 8), &builtin::resolver())
            .unwrap_or_else(|error| panic!("a declared picture must load: {error}"));
        let (first, again) = (pictures.sheet("spinner"), pictures.sheet("spinner"));

        assert_eq!(
            first.map(|sheet| std::ptr::from_ref(sheet.as_ref())),
            again.map(|sheet| std::ptr::from_ref(sheet.as_ref()))
        );
    }

    /// A skin that names a picture nothing answers is a broken skin, not a
    /// skin with one drawing fewer.
    #[kithara::test]
    fn a_picture_the_resolver_cannot_answer_is_an_error() {
        let error =
            Pictures::load(&doc("sprites/missing.png", 8), &builtin::resolver()).unwrap_err();

        assert!(matches!(error, crate::error::UiDocError::NotFound { .. }));
    }

    /// A grid the file does not divide is caught while the skin resolves,
    /// rather than showing torn frames at every draw.
    #[kithara::test]
    fn a_grid_the_picture_does_not_divide_is_an_error() {
        let error =
            Pictures::load(&doc("sprites/spinner.png", 7), &builtin::resolver()).unwrap_err();

        assert!(matches!(error, crate::error::UiDocError::Picture { .. }));
    }

    /// The picture is named beside the skin that declares it, so a skin in a
    /// directory of its own reaches the picture beside it and nothing else.
    #[kithara::test]
    fn a_picture_is_named_beside_the_skin_that_declares_it() {
        let mut document = doc("sprites/spinner.png", 8);
        document
            .rebase(&SourceUri("skins/dark.kskin.ron".to_owned()))
            .unwrap_or_else(|error| panic!("a path beside the skin must rebase: {error}"));

        assert_eq!(
            document.sheets["spinner"].source,
            "skins/sprites/spinner.png"
        );
    }

    fn worn(id: &str) -> &'static Skin {
        builtin::skins()
            .iter()
            .find(|skin| skin.id() == id)
            .unwrap_or_else(|| panic!("the toolkit ships a skin called {id:?}"))
    }

    fn first_frame(id: &str) -> Option<Vec<u8>> {
        worn(id)
            .sheet("spinner")?
            .frame(0)
            .and_then(Image::rgba)
            .map(|pixels| pixels.to_vec())
    }

    /// The whole point of a skin carrying pictures: one document naming one
    /// picture draws two different ones under two skins.
    #[kithara::test]
    fn two_skins_answer_one_name_with_two_pictures() {
        assert_ne!(first_frame("kithara-dark"), first_frame("kithara-neon"));
    }

    /// A skin that restates no picture keeps the ones it is written over,
    /// on the same terms as every colour it leaves alone.
    #[kithara::test]
    fn a_skin_restating_no_picture_keeps_the_ones_it_inherits() {
        assert_eq!(first_frame("kithara-dark"), first_frame("kithara-light"));
    }
}
