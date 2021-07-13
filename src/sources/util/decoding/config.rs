//! Definition of a common decoding configuration which is able to resolve
//! `Decoder` implementations that are registered in the global inventory.

#![deny(missing_docs)]

use super::{BytesDecoder, Decoder};
use crate::event::Value;
use bytes::Bytes;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

lazy_static! {
    /// A hashmap that resolves from shorthand name to `Decoder` implementation.
    ///
    /// The `Decoder` implementations must register themselves in the global
    /// inventory using `inventory::submit!` to be resolved at this point.
    static ref DECODERS: HashMap<&'static str, &'static dyn super::Decoder> =
        inventory::iter::<Box<dyn super::Decoder>>
            .into_iter()
            .map(|decoder| (decoder.name(), &**decoder))
            .collect();
}

/// A decoding configuration, either by shorthand name or fully resolved with
/// options, e.g:
///
/// ```toml
/// # Decoder by shorthand name.
/// decoding = "json"
/// ```
///
/// ```toml
/// # Decoder with options.
/// decoding = { codec = "json", drop_invalid = true }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DecodingConfig {
    /// A shorthand name which is used to resolve the decoder from the
    /// global inventory.
    Name(String),
    /// A fully resolved decoder with options applied.
    Decoder(Box<dyn super::Decoder>),
}

impl DecodingConfig {
    /// Returns the shorthand name of this decoding.
    #[cfg(test)]
    pub fn name(&self) -> String {
        match &self {
            Self::Name(name) => name.into(),
            Self::Decoder(decoder) => decoder.name().to_owned(),
        }
    }
}

pub trait DecodingBuilder {
    /// Builds the transform that converts from byte frame to event value.
    fn build(&self) -> crate::Result<Box<dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync>>;
}

impl DecodingBuilder for DecodingConfig {
    fn build(&self) -> crate::Result<Box<dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync>> {
        match &self {
            Self::Name(name) => match DECODERS.get(name.as_str()) {
                Some(decoder) => decoder.build(),
                _ => Err(format!(r#"Unknown codec "{}""#, name).into()),
            },
            Self::Decoder(decoder) => decoder.build(),
        }
    }
}

impl DecodingBuilder for Option<DecodingConfig> {
    fn build(&self) -> crate::Result<Box<dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync>> {
        match self {
            Some(decoder) => decoder.build(),
            None => BytesDecoder.build(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    /// A helper struct to test deserializing `DecodingConfig` from the context of
    /// a valid TOML configuration.
    #[derive(Debug, Deserialize)]
    struct Config {
        /// The decoding to be tested.
        pub decoding: DecodingConfig,
    }

    #[test]
    fn config_codec() {
        let config: Config = toml::from_str(indoc! {r#"
            decoding = "bytes"
        "#})
        .unwrap();
        let decoding = config.decoding;

        assert_eq!(decoding.name(), "bytes");
    }

    #[test]
    fn config_codec_with_options() {
        let config: Config = toml::from_str(indoc! {r#"
            [decoding]
            codec = "bytes"
        "#})
        .unwrap();
        let decoding = config.decoding;

        assert_eq!(decoding.name(), "bytes");
    }

    #[test]
    fn build_codec() {
        let config: Config = toml::from_str(indoc! {r#"
            decoding = "bytes"
        "#})
        .unwrap();
        let decoding = config.decoding;
        let decoder = decoding.build();

        assert!(decoder.is_ok());
    }

    #[test]
    fn build_codec_unknown() {
        let config: Config = toml::from_str(indoc! {r#"
            decoding = "unknown"
        "#})
        .unwrap();
        let decoding = config.decoding;
        let decoder = decoding.build();

        assert_eq!(
            decoder.err().map(|error| error.to_string()),
            Some(r#"Unknown codec "unknown""#.to_owned())
        );
    }
}
