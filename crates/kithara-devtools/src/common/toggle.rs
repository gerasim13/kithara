use std::{fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, Visitor},
};

pub trait ToggleSchema {
    const FIELDS: &'static [&'static str; 2];
    const DEFAULT: bool;
}

#[derive_where::derive_where(Clone, Debug)]
#[derive(kithara_config::Config)]
#[config(builder(none), fields(value))]
pub struct ToggleConfig<Tag> {
    pub items: Vec<String>,
    pub enabled: bool,
    #[config(skip = "schema marker")]
    #[derive_where(skip(Debug))]
    marker: PhantomData<fn() -> Tag>,
}

impl<Tag: ToggleSchema> Default for ToggleConfig<Tag> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            enabled: Tag::DEFAULT,
            marker: PhantomData,
        }
    }
}

impl<'de, Tag: ToggleSchema> Deserialize<'de> for ToggleConfig<Tag> {
    fn deserialize<Decoder: Deserializer<'de>>(decoder: Decoder) -> Result<Self, Decoder::Error> {
        decoder.deserialize_struct("ToggleConfig", Tag::FIELDS, Self::default())
    }
}

impl<'de, Tag: ToggleSchema> Visitor<'de> for ToggleConfig<Tag> {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a toggle and its item list")
    }

    fn visit_map<Map: MapAccess<'de>>(mut self, mut map: Map) -> Result<Self, Map::Error> {
        let mut items = None;
        let mut enabled = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == Tag::FIELDS[0] {
                if items.is_some() {
                    return Err(Map::Error::duplicate_field(Tag::FIELDS[0]));
                }
                items = Some(map.next_value()?);
            } else if key == Tag::FIELDS[1] {
                if enabled.is_some() {
                    return Err(Map::Error::duplicate_field(Tag::FIELDS[1]));
                }
                enabled = Some(map.next_value()?);
            } else {
                return Err(Map::Error::unknown_field(&key, Tag::FIELDS));
            }
        }
        if let Some(items) = items {
            self.items = items;
        }
        if let Some(enabled) = enabled {
            self.enabled = enabled;
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{ToggleConfig, ToggleSchema};

    enum TestSchema {}

    impl ToggleSchema for TestSchema {
        const FIELDS: &'static [&'static str; 2] = &["features", "default"];
        const DEFAULT: bool = true;
    }

    #[test]
    fn toggles_preserve_schema_defaults_and_reject_other_keys() {
        let defaults: ToggleConfig<TestSchema> = toml::from_str("").expect("empty config");
        assert!(defaults.enabled);
        assert!(defaults.items.is_empty());
        let explicit: ToggleConfig<TestSchema> =
            toml::from_str("features = ['flash']\ndefault = false").expect("explicit config");
        assert!(!explicit.enabled);
        assert_eq!(explicit.items, ["flash"]);
        for invalid in [
            "enabled = false",
            "items = []",
            "default = 'yes'",
            "default = false\ndefault = true",
        ] {
            assert!(toml::from_str::<ToggleConfig<TestSchema>>(invalid).is_err());
        }
    }
}
