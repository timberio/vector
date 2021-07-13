//! Definition of a common `Decoder` trait.

#![deny(missing_docs)]

mod bytes;
mod config;

pub use self::bytes::BytesDecoder;
use crate::event::Value;
use ::bytes::Bytes;
pub use config::{DecodingBuilder, DecodingConfig};

/// The common `Decoder` trait which provides a method to build a transform that
/// converts from byte frame to event value.
///
/// *Note*: Implementations of the `Decoder` trait must register in the global
/// inventory using `inventory::submit!` to be resolved from a configuration.
#[typetag::serde(tag = "codec")]
pub trait Decoder: std::fmt::Debug + Send + Sync + dyn_clone::DynClone {
    /// Returns the name under which this decoder should be resolved.
    fn name(&self) -> &'static str;

    /// Builds the decoder transformation.
    fn build(&self) -> crate::Result<Box<dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync>>;
}

dyn_clone::clone_trait_object!(Decoder);

inventory::collect!(Box<dyn Decoder>);
