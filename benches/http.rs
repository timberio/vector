use criterion::{criterion_group, BatchSize, BenchmarkId, Criterion, SamplingMode, Throughput};
use futures::TryFutureExt;
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Response, Server,
};
use std::net::SocketAddr;
use tokio::runtime::Runtime;
use vector::{
    config, sinks,
    sinks::util::Compression,
    sources,
    test_util::{next_addr, random_lines, runtime, send_lines, start_topology, wait_for_tcp},
    Error,
};

fn benchmark_http(c: &mut Criterion) {
    let num_lines: usize = 1_000;
    let line_size: usize = 100;

    let in_addr = next_addr();
    let out_addr = next_addr();

    let _srv = serve(out_addr);

    let mut group = c.benchmark_group("http");
    group.throughput(Throughput::Bytes((num_lines * line_size) as u64));
    group.sampling_mode(SamplingMode::Flat);

    for compression in [Compression::None, Compression::gzip_default()].iter() {
        group.bench_with_input(
            BenchmarkId::new("compression", compression),
            compression,
            |b, compression| {
                b.iter_batched(
                    || {
                        let mut config = config::Config::builder();
                        config.add_source(
                            "in",
                            sources::socket::SocketConfig::make_basic_tcp_config(in_addr),
                        );
                        config.add_sink(
                            "out",
                            &["in"],
                            sinks::http::HttpSinkConfig {
                                uri: out_addr.to_string().parse::<http::Uri>().unwrap().into(),
                                compression: *compression,
                                method: Default::default(),
                                auth: Default::default(),
                                headers: Default::default(),
                                batch: sinks::util::BatchConfig {
                                    max_bytes: Some(num_lines * line_size),
                                    ..Default::default()
                                },
                                encoding: sinks::http::Encoding::Text.into(),
                                request: Default::default(),
                                tls: Default::default(),
                                proxy: Default::default(),
                            },
                        );

                        let rt = runtime();
                        let topology = rt.block_on(async move {
                            let (topology, _crash) =
                                start_topology(config.build().unwrap(), false).await;
                            wait_for_tcp(in_addr).await;
                            topology
                        });
                        (rt, topology)
                    },
                    |(rt, topology)| {
                        rt.block_on(async move {
                            let lines = random_lines(line_size).take(num_lines);
                            send_lines(in_addr, lines).await.unwrap();
                            topology.stop().await;
                        })
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }

    group.finish();
}

fn serve(addr: SocketAddr) -> Runtime {
    let rt = runtime();
    rt.spawn(async move {
        let make_service = make_service_fn(|_| async {
            Ok::<_, Error>(service_fn(|_req| async {
                Ok::<_, Error>(Response::new(Body::empty()))
            }))
        });

        Server::bind(&addr)
            .serve(make_service)
            .map_err(|e| panic!("{}", e))
            .await
    });
    rt
}

criterion_group!(benches, benchmark_http);
