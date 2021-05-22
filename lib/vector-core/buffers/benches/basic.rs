use buffers::bytes::{DecodeBytes, EncodeBytes};
use buffers::{self, Variant, WhenFull};
use bytes::{Buf, BufMut};
use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BatchSize, BenchmarkGroup, BenchmarkId,
    Criterion, SamplingMode, Throughput,
};
use futures::task::{noop_waker, Context, Poll};
use futures::{Sink, Stream};
use std::pin::Pin;
use std::time::Duration;
use std::{fmt, mem};

#[derive(Clone, Copy)]
struct Message<const N: usize> {
    id: u64,
    _padding: [u64; N],
}

impl<const N: usize> Message<N> {
    fn new(id: u64) -> Self {
        Message {
            id,
            _padding: [0; N],
        }
    }
}

#[derive(Debug)]
enum EncodeError {}

#[derive(Debug)]
enum DecodeError {}

impl fmt::Display for DecodeError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unreachable!()
    }
}

impl<const N: usize> EncodeBytes<Message<N>> for Message<N> {
    type Error = EncodeError;

    fn encode<B>(self, buffer: &mut B) -> Result<(), Self::Error>
    where
        B: BufMut,
        Self: Sized,
    {
        buffer.put_u64(self.id);
        for _ in 0..N {
            // this covers self._padding
            buffer.put_u64(0);
        }
        Ok(())
    }
}

impl<const N: usize> DecodeBytes<Message<N>> for Message<N> {
    type Error = DecodeError;

    fn decode<B>(mut buffer: B) -> Result<Self, Self::Error>
    where
        B: Buf,
        Self: Sized,
    {
        let id = buffer.get_u64();
        for _ in 0..N {
            // this covers self._padding
            let _ = buffer.get_u64();
        }
        Ok(Message::new(id))
    }
}

fn setup<const N: usize>(
    max_events: usize,
) -> (
    Box<dyn Sink<Message<N>, Error = ()> + Unpin + Send>,
    Box<dyn Stream<Item = Message<N>> + Unpin + Send>,
    Vec<Message<N>>,
) {
    let variant = Variant::Memory {
        max_events,
        when_full: WhenFull::DropNewest,
    };
    let mut messages: Vec<Message<N>> = Vec::with_capacity(max_events);
    for i in 0..max_events {
        messages.push(Message::new(i as u64));
    }

    let (tx, rx, _) = buffers::build::<Message<N>>(variant).unwrap();
    (tx.get(), rx, messages)
}

// This function will be used in
fn measurement<const N: usize>(
    mut input: (
        Box<dyn Sink<Message<N>, Error = ()> + Unpin + Send>,
        Box<dyn Stream<Item = Message<N>> + Unpin + Send>,
        Vec<Message<N>>,
    ),
) {
    {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let sink = input.0.as_mut();
        for msg in input.2.into_iter() {
            loop {
                match Sink::poll_ready(Pin::new(sink), &mut context) {
                    Poll::Ready(Ok(())) => match Sink::start_send(Pin::new(sink), msg) {
                        Ok(()) => match Sink::poll_flush(Pin::new(sink), &mut context) {
                            Poll::Ready(Ok(())) => {
                                break;
                            }
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    },
                    _ => unreachable!(),
                }
            }
        }
    }

    {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let stream = input.1.as_mut();
        while let Poll::Ready(Some(_)) = Stream::poll_next(Pin::new(stream), &mut context) {}
    }
}

macro_rules! write_then_read_memory {
    ($criterion:expr, [$( $width:expr ),*]) => {
        let mut group: BenchmarkGroup<WallTime> = $criterion.benchmark_group("buffer");
        group.sampling_mode(SamplingMode::Auto);

        let max_events = 1_000;
        $(
            let bytes = mem::size_of::<Message<$width>>();
            group.throughput(Throughput::Elements(max_events as u64));
            group.bench_with_input(
                BenchmarkId::new("memory/write-only-bytes", bytes),
                &max_events,
                |b, max_events| {
                    b.iter_batched(
                        || setup::<$width>(*max_events),
                        measurement,
                        BatchSize::SmallInput,
                    )
                },
            );
        )*
    };
}

fn write_then_read_memory(c: &mut Criterion) {
    write_then_read_memory!(c, [32, 64, 128, 256, 512, 1024]);
}

criterion_group!(
    name = basic;
    config = Criterion::default().measurement_time(Duration::from_secs(60)).confidence_level(0.99).nresamples(500_000).sample_size(250);
    targets = write_then_read_memory
);
criterion_main!(basic);
