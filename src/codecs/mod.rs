//! Definition of a common `Codec` trait used to build decoders and encoders.

#![deny(missing_docs)]

#[cfg(test)]
mod noop;

use crate::config::DataType;
#[cfg(test)]
pub use noop::NoopCodec;
use vector_core::{event::Event, transform::Transform};

/// The common `Codec` trait which provides methods to build a symmetric pair of
/// a decoder and an encoder, where decoding should have the inverse effect of
/// encoding.
///
/// In case one side of the codec doesn't have an inverse, the other side should
/// return an error when building.
///
/// *Note*: Implementations of the `Codec` trait must register in the global
/// inventory using `inventory::submit!` to be resolved from a configuration.
#[typetag::serde(tag = "type")]
pub trait Codec: std::fmt::Debug + Send + Sync + dyn_clone::DynClone {
    /// Returns the name under which this codec should be resolved.
    fn name(&self) -> &'static str;

    /// Builds the decoder associated to this codec.
    fn build_decoder(&self) -> crate::Result<CodecTransform>;

    /// Builds the encoder associated to this codec.
    fn build_encoder(&self) -> crate::Result<CodecTransform>;
}

dyn_clone::clone_trait_object!(Codec);

/// Struct containing the build result for decoders/encoders.
pub struct CodecTransform {
    /// The type of the input that is accepted by this codec.
    pub input_type: DataType,
    /// The transform operation to apply this codec.
    pub transform: Transform<Event>,
}

inventory::collect!(Box<dyn Codec>);
