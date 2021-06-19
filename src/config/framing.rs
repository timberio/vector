//! Definition of a common framing configuration which is able to resolve
//! `Framer` implementations that are registered in the global inventory.

#![deny(missing_docs)]

use lazy_static::lazy_static;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use vector_core::transform::Transform;

lazy_static! {
    /// A hashmap that resolves from shorthand name to `Framer` implementation.
    ///
    /// The `Framer` implementations must register themselves in the global
    /// inventory using `inventory::submit!` to be resolved at this point.
    static ref FRAMERS: HashMap<&'static str, &'static dyn crate::framers::Framer> =
        inventory::iter::<Box<dyn crate::framers::Framer>>
            .into_iter()
            .map(|framer| (framer.name(), &**framer))
            .collect();
}

/// A collection of framing configurations.
///
/// Framers may be specified alone or as multiple (which will be chained
/// together), by shorthand name only or with options, e.g:
///
/// ```toml
/// # Single framer by shorthand name.
/// framing = "line_delimited"
/// ```
///
/// ```toml
/// # Single framer with options.
/// framing = { type = "character_delimited", character = "\n" }
/// ```
///
/// ```toml
/// # Multiple framers, by shorthand name and with options.
/// framing = ["line_delimited", { type = "character_delimited", character = " " }]
/// ```
///
/// The shorthand name and available options are determined by the `Framer`
/// implementations which are registered in the global inventory.
#[derive(Debug, Clone, Serialize, Default)]
pub struct FramingsConfig(pub Box<[FramingConfig]>);

impl<'de> Deserialize<'de> for FramingsConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// A helper enum to uniformly deserialize one or many framing
        /// configurations.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum FramingsConfig {
            /// Represents a single framing config.
            Single(FramingConfig),
            /// Represents multiple framing configs.
            Multiple(Box<[FramingConfig]>),
        }

        let config = FramingsConfig::deserialize(deserializer)?;

        Ok(Self(match config {
            FramingsConfig::Single(config) => vec![config].into(),
            FramingsConfig::Multiple(configs) => configs,
        }))
    }
}

impl IntoIterator for FramingsConfig {
    type Item = FramingConfig;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        Into::<Vec<_>>::into(self.0).into_iter()
    }
}

impl From<Vec<FramingConfig>> for FramingsConfig {
    fn from(configs: Vec<FramingConfig>) -> Self {
        Self(configs.into())
    }
}

/// A framing configuration, either by shorthand name or fully resolved with
/// options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FramingConfig {
    /// A shorthand name which is used to resolve the framer from the
    /// global inventory.
    Name(String),
    /// A fully resolved framer with options applied.
    Framer(Box<dyn crate::framers::Framer>),
}

impl FramingConfig {
    /// Returns the shorthand name of this framer.
    pub fn name(&self) -> String {
        match &self {
            Self::Name(name) => name.into(),
            Self::Framer(framer) => framer.name().to_owned(),
        }
    }

    /// Builds the transformation associated to this framer.
    pub fn build(&self) -> crate::Result<Transform<Vec<u8>>> {
        match &self {
            Self::Name(name) => match FRAMERS.get(name.as_str()) {
                Some(framer) => framer.build(),
                _ => Err(format!(r#"Unknown framer "{}""#, name).into()),
            },
            Self::Framer(framer) => framer.build(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    /// A helper struct to test deserializing `FramingsConfig` from the context
    /// of a valid TOML configuration.
    #[derive(Debug, Deserialize)]
    struct Config {
        /// The framing to be tested.
        pub framing: FramingsConfig,
    }

    #[test]
    fn config_framers_single() {
        let config: Config = toml::from_str(indoc! {r#"
            framing = "noop"
        "#})
        .unwrap();
        let framers = config.framing.0;

        assert_eq!(framers.len(), 1);
        assert_eq!(framers[0].name(), "noop");
    }

    #[test]
    fn config_framers_multiple() {
        let config: Config = toml::from_str(indoc! {r#"
            framing = ["noop", "noop"]
        "#})
        .unwrap();
        let framers = config.framing.0;

        assert_eq!(framers.len(), 2);
        assert_eq!(framers[0].name(), "noop");
        assert_eq!(framers[1].name(), "noop");
    }

    #[test]
    fn config_framers_with_options() {
        let config: Config = toml::from_str(indoc! {r#"
            [framing]
            type = "noop"
        "#})
        .unwrap();
        let framers = config.framing.0;

        assert_eq!(framers.len(), 1);
        assert_eq!(framers[0].name(), "noop");
    }

    #[test]
    fn config_framers_with_options_multiple() {
        let config: Config = toml::from_str(indoc! {r#"
            [[framing]]
            type = "noop"

            [[framing]]
            type = "noop"
        "#})
        .unwrap();
        let framers = config.framing.0;

        assert_eq!(framers.len(), 2);
        assert_eq!(framers[0].name(), "noop");
        assert_eq!(framers[1].name(), "noop");
    }

    #[test]
    fn build_framer() {
        let config: Config = toml::from_str(indoc! {r#"
            framing = "noop"
        "#})
        .unwrap();
        let framers = config.framing.0;
        let framer = framers[0].build();

        assert!(framer.is_ok());
    }

    #[test]
    fn build_framer_unknown() {
        let config: Config = toml::from_str(indoc! {r#"
            framing = "unknown"
        "#})
        .unwrap();
        let framers = config.framing.0;
        let framer = framers[0].build();

        assert_eq!(
            framer.err().map(|error| error.to_string()),
            Some(r#"Unknown framer "unknown""#.to_owned())
        );
    }
}
