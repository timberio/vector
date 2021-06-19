//! Definition of a common codec configuration which is able to resolve `Codec`
//! implementations that are registered in the global inventory.

#![deny(missing_docs)]

use crate::codecs::CodecTransform;
use lazy_static::lazy_static;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

lazy_static! {
    /// A hashmap that resolves from shorthand name to `Codec` implementation.
    ///
    /// The `Codec` implementations must register themselves in the global
    /// inventory using `inventory::submit!` to be resolved at this point.
    static ref CODECS: HashMap<&'static str, &'static dyn crate::codecs::Codec> =
        inventory::iter::<Box<dyn crate::codecs::Codec>>
            .into_iter()
            .map(|codec| (codec.name(), &**codec))
            .collect();
}

/// A collection of codec configurations.
///
/// Codecs may be specified alone or as multiple (which will be chained
/// together), by shorthand name only or with options, e.g:
///
/// ```toml
/// # Single codec by shorthand name.
/// codec = "json"
/// ```
///
/// ```toml
/// # Single codec with options.
/// codec = { type = "json", target_field = "foo" }
/// ```
///
/// ```toml
/// # Multiple codecs, by shorthand name and with options.
/// codec = ["syslog", { type = "json", target_field = "foo" }]
/// ```
///
/// The shorthand name and available options are determined by the `Codec`
/// implementations which are registered in the global inventory.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CodecsConfig(pub Box<[CodecConfig]>);

impl<'de> Deserialize<'de> for CodecsConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// A helper enum to uniformly deserialize one or many codec
        /// configurations.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CodecsConfig {
            /// Represents a single codec config.
            Single(CodecConfig),
            /// Represents multiple codec configs.
            Multiple(Box<[CodecConfig]>),
        }

        let config = CodecsConfig::deserialize(deserializer)?;

        Ok(Self(match config {
            CodecsConfig::Single(config) => vec![config].into(),
            CodecsConfig::Multiple(configs) => configs,
        }))
    }
}

impl IntoIterator for CodecsConfig {
    type Item = CodecConfig;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        Into::<Vec<_>>::into(self.0).into_iter()
    }
}

impl From<Vec<CodecConfig>> for CodecsConfig {
    fn from(configs: Vec<CodecConfig>) -> Self {
        Self(configs.into())
    }
}

/// A codec configuration, either by shorthand name or fully resolved with
/// options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodecConfig {
    /// A shorthand name which is used to resolve the codec from the
    /// global inventory.
    Name(String),
    /// A fully resolved codec with options applied.
    Codec(Box<dyn crate::codecs::Codec>),
}

impl CodecConfig {
    /// Returns the shorthand name of this codec.
    pub fn name(&self) -> String {
        match &self {
            Self::Name(name) => name.into(),
            Self::Codec(codec) => codec.name().to_owned(),
        }
    }

    /// Builds the decoder associated to this codec.
    pub fn build_decoder(&self) -> crate::Result<CodecTransform> {
        match &self {
            Self::Name(name) => match CODECS.get(name.as_str()) {
                Some(codec) => codec.build_decoder(),
                _ => Err(format!(r#"Unknown codec "{}""#, name).into()),
            },
            Self::Codec(codec) => codec.build_decoder(),
        }
    }

    /// Builds the decoder associated to this codec.
    pub fn build_encoder(&self) -> crate::Result<CodecTransform> {
        match &self {
            Self::Name(name) => match CODECS.get(name.as_str()) {
                Some(codec) => codec.build_encoder(),
                _ => Err(format!(r#"Unknown codec "{}""#, name).into()),
            },
            Self::Codec(codec) => codec.build_encoder(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    /// A helper struct to test deserializing `CodecsConfig` from the context of
    /// a valid TOML configuration.
    #[derive(Debug, Deserialize)]
    struct Config {
        /// The codec to be tested.
        pub codec: CodecsConfig,
    }

    #[test]
    fn config_codecs_single() {
        let config: Config = toml::from_str(indoc! {r#"
            codec = "noop"
        "#})
        .unwrap();
        let codecs = config.codec.0;

        assert_eq!(codecs.len(), 1);
        assert_eq!(codecs[0].name(), "noop");
    }

    #[test]
    fn config_codecs_multiple() {
        let config: Config = toml::from_str(indoc! {r#"
            codec = ["noop", "noop"]
        "#})
        .unwrap();
        let codecs = config.codec.0;

        assert_eq!(codecs.len(), 2);
        assert_eq!(codecs[0].name(), "noop");
        assert_eq!(codecs[1].name(), "noop");
    }

    #[test]
    fn config_codecs_with_options() {
        let config: Config = toml::from_str(indoc! {r#"
            [codec]
            type = "noop"
        "#})
        .unwrap();
        let codecs = config.codec.0;

        assert_eq!(codecs.len(), 1);
        assert_eq!(codecs[0].name(), "noop");
    }

    #[test]
    fn config_codecs_with_options_multiple() {
        let config: Config = toml::from_str(indoc! {r#"
            [[codec]]
            type = "noop"

            [[codec]]
            type = "noop"
        "#})
        .unwrap();
        let codecs = config.codec.0;

        assert_eq!(codecs.len(), 2);
        assert_eq!(codecs[0].name(), "noop");
        assert_eq!(codecs[1].name(), "noop");
    }

    #[test]
    fn build_codec() {
        let config: Config = toml::from_str(indoc! {r#"
            codec = "noop"
        "#})
        .unwrap();
        let codecs = config.codec.0;
        let decoder = codecs[0].build_decoder();
        let encoder = codecs[0].build_encoder();

        assert!(decoder.is_ok());
        assert!(encoder.is_ok());
    }

    #[test]
    fn build_codec_unknown() {
        let config: Config = toml::from_str(indoc! {r#"
            codec = "unknown"
        "#})
        .unwrap();
        let codecs = config.codec.0;
        let decoder = codecs[0].build_decoder();
        let encoder = codecs[0].build_encoder();

        assert_eq!(
            decoder.err().map(|error| error.to_string()),
            Some(r#"Unknown codec "unknown""#.to_owned())
        );
        assert_eq!(
            encoder.err().map(|error| error.to_string()),
            Some(r#"Unknown codec "unknown""#.to_owned())
        );
    }
}
