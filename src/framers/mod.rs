//! Definition of a common `Framer` trait used to frame byte streams.

#![deny(missing_docs)]

#[cfg(test)]
mod noop;

#[cfg(test)]
pub use noop::NoopFramer;
use vector_core::transform::Transform;

/// The common `Framer` trait which provides a method to build a transformation
/// to frame byte streams.
///
/// *Note*: Implementations of the `Framer` trait must register in the global
/// inventory using `inventory::submit!` to be resolved from a configuration.
#[typetag::serde(tag = "type")]
pub trait Framer: std::fmt::Debug + Send + Sync + dyn_clone::DynClone {
    /// Returns the name under which this framer should be resolved.
    fn name(&self) -> &'static str;

    /// Builds the transformation associated to this framer.
    fn build(&self) -> crate::Result<Transform<Vec<u8>>>;
}

dyn_clone::clone_trait_object!(Framer);

inventory::collect!(Box<dyn Framer>);
