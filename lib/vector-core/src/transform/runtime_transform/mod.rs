mod vec_stream;

use crate::{event::Event, transform::TaskTransform};
use futures::{
    stream::{self, BoxStream},
    FutureExt, Stream, StreamExt,
};
use std::{future::ready, pin::Pin};
use tokio::time;
use tokio_stream::wrappers::IntervalStream;
use vec_stream::VecStreamExt;

/// A structure representing user-defined timer.
#[derive(Clone, Copy, Debug)]
pub struct Timer {
    pub id: u32,
    pub interval_seconds: u64,
}

/// A trait representing a runtime running user-defined code.
pub trait RuntimeTransform {
    /// Call user-defined "init" hook.
    fn hook_init<F>(&mut self, _emit_fn: F)
    where
        F: FnMut(Event),
    {
    }

    /// Call user-defined "process" hook.
    fn hook_process<F>(&mut self, event: Event, emit_fn: F)
    where
        F: FnMut(Event);

    /// Call user-defined "shutdown" hook.
    fn hook_shutdown<F>(&mut self, _emit_fn: F)
    where
        F: FnMut(Event),
    {
    }

    /// Call user-defined timer handler.
    fn timer_handler<F>(&mut self, _timer: Timer, _emit_fn: F)
    where
        F: FnMut(Event),
    {
    }

    /// Return (static) list of user-defined timers.
    fn timers(&self) -> Vec<Timer> {
        Vec::new()
    }

    fn transform(&mut self, output: &mut Vec<Event>, event: Event) {
        let mut maybe = None;
        self.hook_process(event, |event| maybe = Some(event));
        output.extend(maybe.into_iter());
    }
}

#[derive(Debug)]
enum Message {
    Init,
    Process(Event),
    Shutdown,
    Timer(Timer),
}

impl<T> TaskTransform<Event> for T
where
    T: RuntimeTransform + Send,
{
    fn transform(
        mut self: Box<Self>,
        input_rx: Pin<Box<dyn Stream<Item = Event> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = Event> + Send>>
    where
        Self: 'static,
    {
        let timers = self.timers();
        let mut is_shutdown: bool = false; // TODO: consider using an enum describing the state instead of a
                                           // a single boolean variable.
                                           // It is used to prevent timers to emit messages after the source
                                           // stream stopped.

        Box::pin(
            input_rx
                .map(Message::Process)
                .fuse()
                .into_future()
                .map(move |(first, rest)| {
                    // The first message is always `Message::Init`.
                    let init_msg = stream::once(ready(Message::Init));
                    // After it comes the first event, if any.
                    let first_event = first.map_or_else(
                        || stream::empty().boxed(),
                        |msg| stream::once(ready(msg)).boxed(),
                    );
                    // Then all other events followed by `Message::Shutdown` message
                    let rest_events_and_shutdown_msg =
                        rest.chain(stream::once(ready(Message::Shutdown)));
                    // A stream of `Message::Timer(..)` events generated by timers.
                    let timer_msgs = make_timer_msgs_stream(timers);

                    init_msg
                        .chain(first_event)
                        .chain(
                            // We need to finish when `rest_events_and_shutdown_msg` finishes so
                            // not to hang on timers, but not finish when `timer_msgs` finishes
                            // as there may not be any timer.
                            rest_events_and_shutdown_msg
                                .select_weak(timer_msgs.chain(stream::pending())),
                        )
                        .boxed()
                })
                .into_stream()
                .flatten()
                .map(move |msg| {
                    let mut acc = Vec::new(); // TODO: create a stream adaptor to avoid buffering all events
                    if !is_shutdown {
                        match msg {
                            Message::Init => self.hook_init(|event| acc.push(event)),
                            Message::Process(event) => {
                                self.hook_process(event, |event| acc.push(event))
                            }
                            Message::Shutdown => {
                                self.hook_shutdown(|event| acc.push(event));
                                is_shutdown = true;
                            }
                            Message::Timer(timer) => {
                                self.timer_handler(timer, |event| acc.push(event))
                            }
                        }
                    }
                    stream::iter(acc).boxed()
                })
                .flatten()
                .boxed(),
        )
    }
}

fn make_timer_msgs_stream(timers: Vec<Timer>) -> BoxStream<'static, Message> {
    let streams = timers.into_iter().map(|timer| {
        let period = time::Duration::from_secs(timer.interval_seconds);
        IntervalStream::new(time::interval(period)).map(move |_| Message::Timer(timer))
    });
    stream::select_all(streams).boxed()
}
