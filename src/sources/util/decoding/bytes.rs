#![deny(missing_docs)]

use super::Decoder;
use crate::event::Value;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// A decoder which wraps the byte frame as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BytesDecoder;

#[typetag::serde(name = "bytes")]
impl Decoder for BytesDecoder {
    fn name(&self) -> &'static str {
        "bytes"
    }

    fn build(&self) -> crate::Result<Box<dyn Fn(Bytes) -> crate::Result<Value> + Send + Sync>> {
        Ok(Box::new(|bytes| Ok(bytes.into())))
    }
}

inventory::submit! {
    Box::new(BytesDecoder) as Box<dyn Decoder>
}
