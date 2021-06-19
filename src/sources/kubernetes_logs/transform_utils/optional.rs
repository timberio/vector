//! Optional transform.

#![deny(missing_docs)]

use crate::{event::Event, transforms::TaskTransform};
use futures::Stream;
use std::pin::Pin;

/// Optional transform.
/// Passes events through the specified transform is any, otherwise passes them,
/// as-is.
/// Useful to avoid boxing the transforms.
#[derive(Clone, Debug)]
pub struct Optional<T: TaskTransform<Event>>(pub Option<T>);

impl<T: TaskTransform<Event>> TaskTransform<Event> for Optional<T> {
    fn transform(
        self: Box<Self>,
        task: Pin<Box<dyn Stream<Item = Event> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Event> + Send>>
    where
        Self: 'static,
    {
        match self.0 {
            Some(val) => Box::new(val).transform(task),
            None => task,
        }
    }
}
