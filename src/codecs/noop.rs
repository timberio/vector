#![deny(missing_docs)]

use super::{Codec, CodecTransform};
use crate::config::DataType;
use serde::{Deserialize, Serialize};
use vector_core::{
    event::Event,
    transform::{FunctionTransform, Transform},
};

/// A codec which returns its input as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoopCodec;

#[typetag::serde(name = "noop")]
impl Codec for NoopCodec {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn build_decoder(&self) -> crate::Result<CodecTransform> {
        Ok(CodecTransform {
            input_type: DataType::Any,
            transform: Transform::function(NoopTransform),
        })
    }

    fn build_encoder(&self) -> crate::Result<CodecTransform> {
        Ok(CodecTransform {
            input_type: DataType::Any,
            transform: Transform::function(NoopTransform),
        })
    }
}

/// A transform which returns its input as-is.
#[derive(Debug, Copy, Clone)]
struct NoopTransform;

impl FunctionTransform<Event> for NoopTransform {
    fn transform(&mut self, output: &mut Vec<Event>, event: Event) {
        output.push(event)
    }
}

inventory::submit! {
    Box::new(NoopCodec) as Box<dyn Codec>
}
