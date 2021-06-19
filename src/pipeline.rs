use crate::{internal_events::EventOut, transforms::FunctionTransform};
use futures::{channel::mpsc, task::Poll, Sink};
#[cfg(test)]
use futures::{Stream, StreamExt};
use std::{collections::VecDeque, fmt, pin::Pin, task::Context};
use vector_core::event::Event;
#[cfg(test)]
use vector_core::event::EventStatus;

#[derive(Debug)]
pub struct ClosedError;

impl fmt::Display for ClosedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pipeline is closed.")
    }
}

impl std::error::Error for ClosedError {}

const MAX_ENQUEUED: usize = 1000;

#[derive(Derivative, Clone)]
#[derivative(Debug)]
pub struct Pipeline {
    inner: mpsc::Sender<Event>,
    // We really just keep this around in case we need to rebuild.
    #[derivative(Debug = "ignore")]
    inlines: Vec<Box<dyn FunctionTransform<Event>>>,
    enqueued: VecDeque<Event>,
}

impl Pipeline {
    fn try_flush(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), <Self as Sink<Event>>::Error>> {
        while let Some(event) = self.enqueued.pop_front() {
            match self.inner.poll_ready(cx) {
                Poll::Pending => {
                    self.enqueued.push_front(event);
                    return Poll::Pending;
                }
                Poll::Ready(Ok(())) => {
                    // continue to send below
                }
                Poll::Ready(Err(_error)) => return Poll::Ready(Err(ClosedError)),
            }

            match self.inner.start_send(event) {
                Ok(()) => {
                    // we good, keep looping
                }
                Err(error) if error.is_full() => {
                    // We only try to send after a successful call to poll_ready, which reserves
                    // space for us in the channel. That makes this branch unreachable as long as
                    // the channel implementation fulfills its own contract.
                    panic!("Channel was both ready and full; this is a bug.")
                }
                Err(error) if error.is_disconnected() => {
                    return Poll::Ready(Err(ClosedError));
                }
                Err(_) => unreachable!(),
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl Sink<Event> for Pipeline {
    type Error = ClosedError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.enqueued.len() < MAX_ENQUEUED {
            Poll::Ready(Ok(()))
        } else {
            self.try_flush(cx)
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: Event) -> Result<(), Self::Error> {
        emit!(EventOut { count: 1 });
        // Note how this gets **swapped** with `new_working_set` in the loop.
        // At the end of the loop, it will only contain finalized events.
        let mut working_set = vec![item];
        for inline in self.inlines.iter_mut() {
            let mut new_working_set = Vec::with_capacity(working_set.len());
            for event in working_set.drain(..) {
                inline.transform(&mut new_working_set, event);
            }
            core::mem::swap(&mut new_working_set, &mut working_set);
        }
        self.enqueued.extend(working_set);
        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.try_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

impl Pipeline {
    #[cfg(test)]
    pub fn new_test() -> (Self, mpsc::Receiver<Event>) {
        Self::new_with_buffer(100, vec![])
    }

    #[cfg(test)]
    pub fn new_test_finalize(status: EventStatus) -> (Self, impl Stream<Item = Event> + Unpin) {
        let (pipe, recv) = Self::new_with_buffer(100, vec![]);
        // In a source test pipeline, there is no sink to acknowledge
        // events, so we have to add a map to the receiver to handle the
        // finalization.
        let recv = recv.map(move |mut event| {
            let metadata = event.metadata_mut();
            metadata.update_status(status);
            metadata.update_sources();
            event
        });
        (pipe, recv)
    }

    pub fn new_with_buffer(
        n: usize,
        inlines: Vec<Box<dyn FunctionTransform<Event>>>,
    ) -> (Self, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel(n);
        (Self::from_sender(tx, inlines), rx)
    }

    pub fn from_sender(
        inner: mpsc::Sender<Event>,
        inlines: Vec<Box<dyn FunctionTransform<Event>>>,
    ) -> Self {
        Self {
            inner,
            inlines,
            // We ensure the buffer is sufficient that it is unlikely to require reallocations.
            // There is a possibility a component might blow this queue size.
            enqueued: VecDeque::with_capacity(10),
        }
    }
}

#[cfg(all(test, feature = "transforms-add_fields", feature = "transforms-filter"))]
mod test {
    use super::Pipeline;
    use crate::{
        event::{Event, Value},
        test_util::collect_ready,
        transforms::{add_fields::AddFields, filter::Filter},
    };
    use futures::SinkExt;
    use serde_json::json;
    use std::convert::TryFrom;

    const KEYS: [&str; 2] = ["booper", "swooper"];

    const VALS: [&str; 2] = ["Pineapple", "Coconut"];

    #[tokio::test]
    async fn multiple_transforms() -> Result<(), crate::Error> {
        let transform_1 = AddFields::new(
            indexmap::indexmap! {
                KEYS[0].into() => Value::from(VALS[0]),
            },
            false,
        )?;
        let transform_2 = AddFields::new(
            indexmap::indexmap! {
                KEYS[1].into() => Value::from(VALS[1]),
            },
            false,
        )?;

        let (mut pipeline, receiver) =
            Pipeline::new_with_buffer(100, vec![Box::new(transform_1), Box::new(transform_2)]);

        let event = Event::try_from(json!({
            "message": "MESSAGE_MARKER",
        }))?;

        pipeline.send(event).await?;
        let out = collect_ready(receiver).await;

        assert_eq!(out[0].as_log().get(KEYS[0]), Some(&Value::from(VALS[0])));
        assert_eq!(out[0].as_log().get(KEYS[1]), Some(&Value::from(VALS[1])));

        Ok(())
    }

    #[tokio::test]
    async fn filtered_output() -> Result<(), crate::Error> {
        let transform_1 = Filter::new(Box::new(crate::conditions::check_fields::CheckFields::new(
            indexmap::indexmap! {
                KEYS[1].into() => crate::conditions::check_fields::EqualsPredicate::new(
                    "message".into(),
                    &crate::conditions::check_fields::CheckFieldsPredicateArg::String("NOT".into()),
                )?,
            },
        )));

        let (mut pipeline, receiver) = Pipeline::new_with_buffer(100, vec![Box::new(transform_1)]);

        let event = Event::try_from(json!({
            "message": "MESSAGE_MARKER",
        }))?;

        pipeline.send(event).await?;
        let out = collect_ready(receiver).await;

        assert_eq!(out, vec![]);

        Ok(())
    }
}
