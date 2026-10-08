/// The global bar's preset picker.
#[derive(kithara_derive::Control)]
#[control(size = skin.global_bar.preset_size)]
pub(crate) struct Preset;

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use consts::ITEMS;

    use super::Preset;
    use crate::{
        atoms::bar::preset::{Preset as Face, PresetData, PresetItem},
        builtin,
        hosts::controls::{Draws, Grip, IndexEvent, Reading},
        render::{ControlAction, ReadValue, Skin, document::Ctx},
    };

    mod consts {
        use super::{PresetItem, builtin};

        pub(super) const ITEMS: [PresetItem; 2] = [
            PresetItem {
                label: "MICRO",
                name: builtin::MICRO_PRESET,
            },
            PresetItem {
                label: "PLAYER",
                name: builtin::PLAYER_PRESET,
            },
        ];
    }

    impl Draws for Preset {
        type Painter = Face;

        fn data(&self, read: Reading<'_>) -> Option<PresetData> {
            Some(Self::snapshot(read.ctx))
        }

        fn grip(&self, _skin: &Skin, data: &PresetData) -> Grip {
            Grip::Index {
                count: data.items.len(),
            }
        }

        fn index_event(&self) -> Option<IndexEvent<PresetData>> {
            Some(select)
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(skin)
        }
    }

    impl Preset {
        pub(crate) fn snapshot(ctx: Ctx<'_, '_>) -> PresetData {
            let items = &ITEMS;
            let active = Self::active(items, ctx);
            PresetData { items, active }
        }

        pub(crate) fn active(items: &[PresetItem], ctx: Ctx<'_, '_>) -> Option<usize> {
            let Some(ReadValue::Text(name)) = ctx.get("ui.preset") else {
                return None;
            };
            items.iter().position(|item| item.name == name)
        }
    }

    fn select(data: &PresetData, index: usize) -> Option<ControlAction> {
        data.items
            .get(index)
            .map(|item| ControlAction::Text(item.name.to_owned()))
    }

    #[cfg(test)]
    mod tests {
        use kithara_test_utils::kithara;

        use super::*;
        use crate::{
            builtin,
            render::{Reads, document::probe},
        };

        struct PresetReads(Option<ReadValue<'static>>);

        impl Reads for PresetReads {
            fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
                if endpoint == "ui.preset" {
                    self.0
                } else {
                    None
                }
            }
        }

        #[kithara::test]
        fn data_keeps_the_canonical_inventory_and_reads_the_active_name() {
            let data = Preset::snapshot(probe(&PresetReads(Some(ReadValue::Text(
                builtin::PLAYER_PRESET,
            )))));

            assert_eq!(data.items.len(), 2);
            assert_eq!(data.items[0].label, "MICRO");
            assert_eq!(data.items[0].name, builtin::MICRO_PRESET);
            assert_eq!(data.items[1].label, "PLAYER");
            assert_eq!(data.items[1].name, builtin::PLAYER_PRESET);
            assert_eq!(data.active, Some(1));
        }

        #[kithara::test]
        fn an_absent_wrong_or_unknown_read_has_no_active_item() {
            for reads in [
                PresetReads(None),
                PresetReads(Some(ReadValue::Bool(true))),
                PresetReads(Some(ReadValue::Text("unknown.klayout.ron"))),
            ] {
                assert_eq!(Preset::snapshot(probe(&reads)).active, None);
            }
        }

        #[kithara::test]
        fn selection_reads_the_name_from_the_same_data_item() {
            let data = Preset::snapshot(probe(&PresetReads(None)));

            assert_eq!(
                select(&data, 0),
                Some(ControlAction::Text(builtin::MICRO_PRESET.to_owned()))
            );
            assert_eq!(
                select(&data, 1),
                Some(ControlAction::Text(builtin::PLAYER_PRESET.to_owned()))
            );
            assert_eq!(select(&data, 2), None);
        }
    }
}
