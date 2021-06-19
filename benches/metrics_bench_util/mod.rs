//! This mod contains the benchmarks for metrics.
//! Due to how metrics are set up, we need isolated process environments to
//! test with metrics core on and off, so the implementation is shared but has
//! two separate entrypoits.
//!
//! # Usage
//!
//! To run these benches, use the following commands:
//!
//!     cargo bench --no-default-features --features "metrics-benches" --bench metrics_on
//!     cargo bench --no-default-features --features "metrics-benches" --bench metrics_off
//!
//! These will run benches with metrics core system on and off respectively.

use criterion::{BatchSize, BenchmarkId, Criterion, SamplingMode, Throughput};
use futures::{future, stream, StreamExt};
use metrics::counter;
use tracing::{span, Level};
use vector::{
    config, sinks, sources,
    test_util::{
        next_addr, random_lines, runtime, send_lines, start_topology, wait_for_tcp, CountReceiver,
    },
};

#[inline]
fn disable_metrics() {
    std::env::set_var("DISABLE_INTERNAL_METRICS_CORE", "true");
}

#[inline]
fn disable_metrics_tracing_integration() {
    std::env::set_var("DISABLE_INTERNAL_METRICS_TRACING_INTEGRATION", "true");
}

#[inline]
fn boot() {
    vector::trace::init(false, false, "warn", false);
    vector::metrics::init().expect("metrics initialization failed");
}

#[allow(dead_code)] // condition compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    MetricsOff,
    MetricsNoTracingIntegration,
    MetricsOn,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::MetricsOff => "metrics/off",
            Mode::MetricsNoTracingIntegration => "metrics/no_tracing_integration",
            Mode::MetricsOn => "metrics/on",
        }
    }
}

/// Due to the nature of how metrics and tracing systems are set up, we need
/// separate process spaces to measure with and without them being enabled.
#[inline]
pub fn benchmark(c: &mut Criterion, mode: Mode) {
    match mode {
        Mode::MetricsOff => {
            disable_metrics();
            disable_metrics_tracing_integration();
        }
        Mode::MetricsNoTracingIntegration => {
            disable_metrics_tracing_integration();
        }
        Mode::MetricsOn => {}
    }
    boot();
    let metrics_core_enabled = mode != Mode::MetricsOff;
    assert_eq!(
        vector::metrics::get_controller().is_ok(),
        metrics_core_enabled,
        "the presence of a controller must correspond to whether metrics core is on or off"
    );

    let bench_name = mode.as_str();

    bench_topology(c, bench_name);
    bench_micro(c, bench_name);
}

/// Based on the topology benchmarks.
/// I'm pretty sure criterion used incorrectly here - the bench loop taps into
/// networking - this must not be working right, as criterion assumptions about
/// the benchmark loop iteration payload are broken here.
#[inline]
fn bench_topology(c: &mut Criterion, bench_name: &'static str) {
    let num_lines: usize = 10_000;
    let line_size: usize = 100;

    let in_addr = next_addr();
    let out_addr = next_addr();

    let mut group = c.benchmark_group(format!("{}/{}", bench_name, "topology"));
    group.sampling_mode(SamplingMode::Flat);
    // Encapsulate noise seen in
    // https://github.com/timberio/vector/runs/1746002475
    group.noise_threshold(0.10);

    for &num_writers in [1, 2, 4, 8, 16].iter() {
        group.throughput(Throughput::Bytes(
            (num_lines * line_size * num_writers) as u64,
        ));
        group.bench_with_input(
            BenchmarkId::new(
                "tcp_socket",
                format!(
                    "{:02}_{}",
                    num_writers,
                    if num_writers == 1 {
                        "writer"
                    } else {
                        "writers"
                    }
                ),
            ),
            &num_writers,
            |b, &num_writers| {
                b.iter_batched(
                    || {
                        let input_lines: Vec<Vec<String>> = (0..num_writers)
                            .into_iter()
                            .map(|_| random_lines(line_size).take(num_lines).collect())
                            .collect();

                        let mut config = config::Config::builder();
                        config.add_source(
                            "in",
                            sources::socket::SocketConfig::make_basic_tcp_config(in_addr),
                            None,
                            None,
                        );
                        config.add_sink(
                            "out",
                            &["in"],
                            sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                                out_addr.to_string(),
                            ),
                        );

                        let rt = runtime();
                        let (output_lines, topology) = rt.block_on(async move {
                            let output_lines = CountReceiver::receive_lines(out_addr);
                            let (topology, _crash) =
                                start_topology(config.build().unwrap(), false).await;
                            wait_for_tcp(in_addr).await;
                            (output_lines, topology)
                        });

                        (input_lines, rt, topology, output_lines)
                    },
                    |(input_lines, rt, topology, output_lines)| {
                        rt.block_on(async move {
                            let sends = stream::iter(input_lines)
                                .map(|lines| send_lines(in_addr, lines))
                                .collect::<Vec<_>>()
                                .await;
                            future::try_join_all(sends).await.unwrap();

                            topology.stop().await;

                            let output_lines = output_lines.await;

                            debug_assert_eq!(num_lines * num_writers, output_lines.len());

                            output_lines
                        });
                    },
                    BatchSize::PerIteration,
                );
            },
        );
    }

    group.finish();
}

/// Here we perform some microbenchmarks on the metrics.
#[inline]
fn bench_micro(c: &mut Criterion, bench_name: &'static str) {
    let mut group = c.benchmark_group(format!("{}/{}", bench_name, "micro"));
    // https://github.com/timberio/vector/runs/1746002475
    group.noise_threshold(0.05);
    group.bench_function("bare_counter", |b| {
        b.iter(|| {
            counter!("test", 1);
        });
    });
    group.bench_function("bare_counter_with_static_labels", |b| {
        b.iter(|| {
            counter!("test", 1, "my key" => "my value");
        });
    });
    group.bench_function("bare_counter_with_dynamic_labels", |b| {
        b.iter_batched(
            || "my value".to_owned(),
            |my_value| {
                counter!("test", 1, "my key" => my_value);
            },
            BatchSize::SmallInput,
        );
    });
    // A span that's not even entered.
    group.bench_function("ununsed_span", |b| {
        b.iter_batched_ref(
            || span!(Level::ERROR, "my span"),
            |_span| {
                counter!("test", 1);
            },
            BatchSize::SmallInput,
        );
    });
    // A span that's entered but without a counter invocation.
    group.bench_function("span_enter_without_counter", |b| {
        b.iter_batched_ref(
            || span!(Level::ERROR, "my span"),
            |span| {
                let _enter = span.enter();
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("span_no_labels", |b| {
        b.iter_batched_ref(
            || span!(Level::ERROR, "my span"),
            |span| {
                let _enter = span.enter();
                counter!("test", 1);
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("span_with_1_static_label", |b| {
        b.iter_batched_ref(
            || span!(Level::ERROR, "my span", "my key" = "my value"),
            |span| {
                let _enter = span.enter();
                counter!("test", 1);
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("span_with_2_static_labels", |b| {
        b.iter_batched_ref(
            || {
                span!(
                    Level::ERROR,
                    "my span",
                    "my key 1" = "my value 1",
                    "my key 2" = "my value 2"
                )
            },
            |span| {
                let _enter = span.enter();
                counter!("test", 1);
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("span_with_1_dynamic_label", |b| {
        b.iter_batched_ref(
            || {
                let my_value = "my value".to_owned();
                span!(Level::ERROR, "my span", "my key" = %my_value)
            },
            |span| {
                let _enter = span.enter();
                counter!("test", 1);
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("span_with_2_dynamic_labels", |b| {
        b.iter_batched_ref(
            || {
                let my_value_1 = "my value 1".to_owned();
                let my_value_2 = "my value 2".to_owned();
                span!(
                    Level::ERROR,
                    "my span",
                    "my key 1" = %my_value_1,
                    "my key 2" = %my_value_2
                )
            },
            |span| {
                let _enter = span.enter();
                counter!("test", 1);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}
