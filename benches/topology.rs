use criterion::{criterion_group, BatchSize, Criterion, SamplingMode, Throughput};
use futures::{future, stream, StreamExt};
use indoc::indoc;
use rand::{rngs::SmallRng, thread_rng, Rng, SeedableRng};

use vector::{
    config, sinks, sources,
    test_util::{
        next_addr, random_lines, runtime, send_lines, start_topology, wait_for_tcp, CountReceiver,
    },
    transforms,
};

fn benchmark_simple_pipes(c: &mut Criterion) {
    let in_addr = next_addr();
    let out_addr = next_addr();

    let mut group = c.benchmark_group("topology");
    group.sampling_mode(SamplingMode::Flat);

    let benchmarks = [
        ("simple", 10_000, 100, 1),
        ("small_lines", 10_000, 1, 1),
        ("big_lines", 2_000, 10_000, 1),
        ("multiple_writers", 1_000, 100, 10),
    ];

    for (name, num_lines, line_size, num_writers) in benchmarks.iter() {
        group.throughput(Throughput::Bytes((num_lines * line_size) as u64));
        group.bench_function(format!("pipe/{}", name), |b| {
            b.iter_batched(
                || {
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
                    (rt, topology, output_lines)
                },
                |(rt, topology, output_lines)| {
                    rt.block_on(async move {
                        let sends = stream::iter(0..*num_writers)
                            .map(|_| {
                                let lines = random_lines(*line_size).take(*num_lines);
                                send_lines(in_addr, lines)
                            })
                            .collect::<Vec<_>>()
                            .await;
                        future::try_join_all(sends).await.unwrap();

                        topology.stop().await;

                        let output_lines = output_lines.await;

                        debug_assert_eq!(*num_lines * num_writers, output_lines.len());

                        output_lines
                    });
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

fn benchmark_interconnected(c: &mut Criterion) {
    let num_lines: usize = 10_000;
    let line_size: usize = 100;

    let in_addr1 = next_addr();
    let in_addr2 = next_addr();
    let out_addr1 = next_addr();
    let out_addr2 = next_addr();

    let mut group = c.benchmark_group("topology");
    group.throughput(Throughput::Bytes((num_lines * line_size * 2) as u64));
    group.sampling_mode(SamplingMode::Flat);

    group.bench_function("interconnected", |b| {
        b.iter_batched(
            || {
                let mut config = config::Config::builder();
                config.add_source(
                    "in1",
                    sources::socket::SocketConfig::make_basic_tcp_config(in_addr1),
                    None,
                    None,
                );
                config.add_source(
                    "in2",
                    sources::socket::SocketConfig::make_basic_tcp_config(in_addr2),
                    None,
                    None,
                );
                config.add_sink(
                    "out1",
                    &["in1", "in2"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr1.to_string()),
                );
                config.add_sink(
                    "out2",
                    &["in1", "in2"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr2.to_string()),
                );

                let rt = runtime();
                let (output_lines1, output_lines2, topology) = rt.block_on(async move {
                    let output_lines1 = CountReceiver::receive_lines(out_addr1);
                    let output_lines2 = CountReceiver::receive_lines(out_addr2);
                    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
                    wait_for_tcp(in_addr1).await;
                    wait_for_tcp(in_addr2).await;
                    (output_lines1, output_lines2, topology)
                });
                (rt, topology, output_lines1, output_lines2)
            },
            |(rt, topology, output_lines1, output_lines2)| {
                rt.block_on(async move {
                    let lines1 = random_lines(line_size).take(num_lines);
                    send_lines(in_addr1, lines1).await.unwrap();
                    let lines2 = random_lines(line_size).take(num_lines);
                    send_lines(in_addr2, lines2).await.unwrap();

                    topology.stop().await;

                    let output_lines1 = output_lines1.await;
                    let output_lines2 = output_lines2.await;

                    debug_assert_eq!(num_lines * 2, output_lines1.len());
                    debug_assert_eq!(num_lines * 2, output_lines2.len());

                    (output_lines1, output_lines2)
                });
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

fn benchmark_transforms(c: &mut Criterion) {
    let num_lines: usize = 10_000;
    let line_size: usize = 100;

    let in_addr = next_addr();
    let out_addr = next_addr();

    let mut group = c.benchmark_group("topology");
    group.throughput(Throughput::Bytes(
        (num_lines * (line_size + "status=404".len())) as u64,
    ));
    group.sampling_mode(SamplingMode::Flat);

    group.bench_function("transforms", |b| {
        b.iter_batched(
            || {
                let mut config = config::Config::builder();
                config.add_source(
                    "in",
                    sources::socket::SocketConfig::make_basic_tcp_config(in_addr),
                    None,
                    None,
                );
                config.add_transform(
                    "parser",
                    &["in"],
                    transforms::regex_parser::RegexParserConfig {
                        patterns: vec![r"status=(?P<status>\d+)".to_string()],
                        field: None,
                        ..Default::default()
                    },
                );
                config.add_transform(
                    "filter",
                    &["parser"],
                    transforms::field_filter::FieldFilterConfig {
                        field: "status".to_string(),
                        value: "404".to_string(),
                    },
                );
                config.add_sink(
                    "out",
                    &["filter"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr.to_string()),
                );

                let rt = runtime();
                let (output_lines, topology) = rt.block_on(async move {
                    let output_lines = CountReceiver::receive_lines(out_addr);
                    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
                    wait_for_tcp(in_addr).await;
                    (output_lines, topology)
                });
                (rt, topology, output_lines)
            },
            |(rt, topology, output_lines)| {
                rt.block_on(async move {
                    let lines = random_lines(line_size)
                        .map(|l| l + "status=404")
                        .take(num_lines);
                    send_lines(in_addr, lines).await.unwrap();

                    topology.stop().await;

                    let output_lines = output_lines.await;

                    debug_assert_eq!(num_lines, output_lines.len());

                    output_lines
                });
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

fn benchmark_complex(c: &mut Criterion) {
    let num_lines: usize = 100_000;
    let sample_rate: u64 = 10;

    let in_addr1 = next_addr();
    let in_addr2 = next_addr();
    let out_addr_all = next_addr();
    let out_addr_sampled = next_addr();
    let out_addr_200 = next_addr();
    let out_addr_404 = next_addr();
    let out_addr_500 = next_addr();

    let mut group = c.benchmark_group("topology");
    group.sampling_mode(SamplingMode::Flat);

    group.bench_function("complex", |b| {
        b.iter_batched(
            || {
                let mut config = config::Config::builder();
                config.add_source(
                    "in1",
                    sources::socket::SocketConfig::make_basic_tcp_config(in_addr1),
                    None,
                    None,
                );
                config.add_source(
                    "in2",
                    sources::socket::SocketConfig::make_basic_tcp_config(in_addr2),
                    None,
                    None,
                );
                config.add_transform(
                    "parser",
                    &["in1", "in2"],
                    transforms::regex_parser::RegexParserConfig {
                        patterns: vec![r"status=(?P<status>\d+)".to_string()],
                        drop_field: false,
                        field: None,
                        ..Default::default()
                    },
                );
                config.add_transform(
                    "filter_200",
                    &["parser"],
                    transforms::field_filter::FieldFilterConfig {
                        field: "status".to_string(),
                        value: "200".to_string(),
                    },
                );
                config.add_transform(
                    "filter_404",
                    &["parser"],
                    transforms::field_filter::FieldFilterConfig {
                        field: "status".to_string(),
                        value: "404".to_string(),
                    },
                );
                config.add_transform(
                    "filter_500",
                    &["parser"],
                    transforms::field_filter::FieldFilterConfig {
                        field: "status".to_string(),
                        value: "500".to_string(),
                    },
                );
                config.add_transform(
                    "sample",
                    &["parser"],
                    transforms::sample::SampleConfig {
                        rate: sample_rate,
                        key_field: None,
                        exclude: None,
                    },
                );
                config.add_sink(
                    "out_all",
                    &["parser"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_all.to_string(),
                    ),
                );
                config.add_sink(
                    "out_sampled",
                    &["sample"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_sampled.to_string(),
                    ),
                );
                config.add_sink(
                    "out_200",
                    &["filter_200"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_200.to_string(),
                    ),
                );
                config.add_sink(
                    "out_404",
                    &["filter_404"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_404.to_string(),
                    ),
                );
                config.add_sink(
                    "out_500",
                    &["filter_500"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_500.to_string(),
                    ),
                );

                let rt = runtime();
                let (
                    output_lines_all,
                    output_lines_sampled,
                    output_lines_200,
                    output_lines_404,
                    output_lines_500,
                    topology,
                ) = rt.block_on(async move {
                    let output_lines_all = CountReceiver::receive_lines(out_addr_all);
                    let output_lines_sampled = CountReceiver::receive_lines(out_addr_sampled);
                    let output_lines_200 = CountReceiver::receive_lines(out_addr_200);
                    let output_lines_404 = CountReceiver::receive_lines(out_addr_404);
                    let output_lines_500 = CountReceiver::receive_lines(out_addr_500);
                    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
                    wait_for_tcp(in_addr1).await;
                    wait_for_tcp(in_addr2).await;
                    (
                        output_lines_all,
                        output_lines_sampled,
                        output_lines_200,
                        output_lines_404,
                        output_lines_500,
                        topology,
                    )
                });
                (
                    rt,
                    topology,
                    output_lines_all,
                    output_lines_sampled,
                    output_lines_200,
                    output_lines_404,
                    output_lines_500,
                )
            },
            |(
                rt,
                topology,
                output_lines_all,
                output_lines_sampled,
                output_lines_200,
                output_lines_404,
                output_lines_500,
            )| {
                rt.block_on(async move {
                    // One sender generates pure random lines
                    let lines1 = random_lines(100).take(num_lines);
                    send_lines(in_addr1, lines1).await.unwrap();

                    // The other includes either status=200 or status=404
                    let mut rng = SmallRng::from_rng(thread_rng()).unwrap();
                    let lines2 = random_lines(100)
                        .map(move |mut l| {
                            let status = if rng.gen_bool(0.5) { "200" } else { "404" };
                            l += "status=";
                            l += status;
                            l
                        })
                        .take(num_lines);
                    send_lines(in_addr2, lines2).await.unwrap();

                    topology.stop().await;

                    let output_lines_all = output_lines_all.await.len();
                    let output_lines_sampled = output_lines_sampled.await.len();
                    let output_lines_200 = output_lines_200.await.len();
                    let output_lines_404 = output_lines_404.await.len();
                    let output_lines_500 = output_lines_500.await.len();

                    debug_assert_eq!(output_lines_all, num_lines * 2);
                    #[cfg(debug_assertions)]
                    {
                        use approx::assert_relative_eq;

                        // binomial distribution
                        let sample_stdev = (output_lines_all as f64
                            * (1f64 / sample_rate as f64)
                            * (1f64 - (1f64 / sample_rate as f64)))
                            .sqrt();

                        assert_relative_eq!(
                            output_lines_sampled as f64,
                            output_lines_all as f64 * (1f64 / sample_rate as f64),
                            epsilon = sample_stdev * 4f64 // should cover 99.993666% of cases
                        );
                    }
                    debug_assert!(output_lines_200 > 0);
                    debug_assert!(output_lines_404 > 0);
                    debug_assert!(output_lines_500 == 0);
                    debug_assert_eq!(output_lines_200 + output_lines_404, num_lines);

                    (
                        output_lines_all,
                        output_lines_sampled,
                        output_lines_200,
                        output_lines_404,
                        output_lines_500,
                    )
                });
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

fn benchmark_real_world_1(c: &mut Criterion) {
    let num_lines: usize = 100_000;

    let in_addr = next_addr();
    let out_addr_company_api = next_addr();
    let out_addr_company_admin = next_addr();
    let out_addr_company_media_proxy = next_addr();
    let out_addr_company_unfurler = next_addr();
    let out_addr_audit = next_addr();

    let mut group = c.benchmark_group("topology");
    group.sampling_mode(SamplingMode::Flat);
    group.throughput(Throughput::Elements(num_lines as u64));
    group.bench_function("real_world_1", |b| {
        b.iter_batched(
            || {
                let mut config = config::Config::builder();
                config.add_source(
                    "in",
                    sources::syslog::SyslogConfig::from_mode(sources::syslog::Mode::Tcp {
                        address: in_addr.into(),
                        keepalive: None,
                        tls: None,
                        receive_buffer_bytes: None,
                    }),
                    None,
                    None,
                );

                let toml_cfg = indoc! {r#"
                    ##
                    ## company-api
                    ##

                    [transforms.company_api]
                    type = "field_filter"
                    inputs = ["in"]
                    field = "appname"
                    value = "company-api"

                    [transforms.company_api_json]
                    type = "json_parser"
                    inputs = ["company_api"]
                    drop_invalid = true

                    [transforms.company_api_timestamp]
                    type = "split"
                    inputs = ["company_api_json"]
                    field = "timestamp"
                    field_names = ["timestamp"]
                    separator = "."

                    [transforms.company_api_timestamp.types]
                    timestamp = "timestamp|%s"

                    [transforms.company_api_metadata]
                    type = "lua"
                    inputs = ["company_api_timestamp"]
                    source = """
                    event["metadata_trace_id"] = event["metadata.trace_id"]
                    event["metadata_guild_id"] = event["metadata.guild_id"]
                    event["metadata_channel_id"] = event["metadata.channel_id"]
                    event["metadata_method"] = event["metadata.method"]
                    """

                    [transforms.company_api_rename]
                    type = "rename_fields"
                    inputs = ["company_api_metadata"]

                    [transforms.company_api_rename.fields]
                    timestamp = "time"
                    host = "hostname"
                    # "metadata.trace_id" = "metadata_trace_id"
                    # "metadata.guild_id" = "metadata_guild_id"
                    # "metadata.channel_id" = "metadata_channel_id"
                    # "metadata.method" = "metadata_method"

                    ##
                    ## company-admin
                    ##

                    [transforms.company_admin]
                    type = "field_filter"
                    inputs = ["in"]
                    field = "appname"
                    value = "company-admin"

                    [transforms.company_admin_json]
                    type = "json_parser"
                    inputs = ["company_admin"]
                    drop_invalid = true

                    [transforms.company_admin_timestamp]
                    type = "split"
                    inputs = ["company_admin_json"]
                    field = "timestamp"
                    field_names = ["timestamp"]
                    separator = "."

                    [transforms.company_admin_timestamp.types]
                    timestamp = "timestamp|%s"

                    [transforms.company_admin_metadata]
                    type = "lua"
                    inputs = ["company_admin_timestamp"]
                    source = """
                    event["metadata_trace_id"] = event["metadata.trace_id"]
                    event["metadata_method"] = event["metadata.method"]
                    """

                    [transforms.company_admin_rename]
                    type = "rename_fields"
                    inputs = ["company_admin_metadata"]

                    [transforms.company_admin_rename.fields]
                    timestamp = "time"
                    host = "hostname"
                    # "metadata.trace_id" = "metadata_trace_id"
                    # "metadata.method" = "metadata_method"

                    ##
                    ## company-media-proxy
                    ##

                    [transforms.company_media_proxy]
                    type = "field_filter"
                    inputs = ["in"]
                    field = "appname"
                    value = "company-media-proxy"

                    [transforms.company_media_proxy_json]
                    type = "json_parser"
                    inputs = ["company_media_proxy"]
                    drop_invalid = true

                    [transforms.company_media_proxy_timestamp]
                    type = "split"
                    inputs = ["company_media_proxy_json"]
                    field = "ts"
                    field_names = ["ts"]
                    separator = "."

                    [transforms.company_media_proxy_timestamp.types]
                    ts = "timestamp|%s"

                    [transforms.company_media_proxy_rename]
                    type = "rename_fields"
                    inputs = ["company_media_proxy_timestamp"]

                    [transforms.company_media_proxy_rename.fields]
                    ts = "time"
                    host = "hostname"

                    ##
                    ## company-unfurler
                    ##

                    [transforms.company_unfurler]
                    type = "field_filter"
                    inputs = ["in"]
                    field = "appname"
                    value = "company-unfurler"

                    [transforms.company_unfurler_hostname]
                    type = "rename_fields"
                    inputs = ["company_unfurler"]

                    [transforms.company_unfurler_hostname.fields]
                    host = "hostname"

                    [transforms.company_unfurler_json]
                    type = "json_parser"
                    inputs = ["company_unfurler_hostname"]
                    drop_invalid = true

                    [transforms.company_unfurler_timestamp]
                    type = "coercer"
                    inputs = ["company_unfurler_json"]

                    [transforms.company_unfurler_timestamp.types]
                    ts = "timestamp"

                    [transforms.company_unfurler_rename]
                    type = "rename_fields"
                    inputs = ["company_unfurler_timestamp"]

                    [transforms.company_unfurler_rename.fields]
                    ts = "time"

                    [transforms.company_unfurler_filter]
                    type = "field_filter"
                    inputs = ["company_unfurler_rename"]
                    field = "msg"
                    value = "unfurl"

                    ##
                    ## audit
                    ##

                    [transforms.audit]
                    type = "field_filter"
                    inputs = ["in"]
                    field = "appname"
                    value = "audit"

                    [transforms.audit_timestamp]
                    type = "coercer"
                    inputs = ["audit"]

                    [transforms.audit_timestamp.types]
                    timestamp = "timestamp"

                    [transforms.audit_rename]
                    type = "rename_fields"
                    inputs = ["audit_timestamp"]

                    [transforms.audit_rename.fields]
                    appname = "tag"
                    host = "hostname"
                    message = "content"
                    timestamp = "time"
                "#};

                let parsed =
                    config::format::deserialize(toml_cfg, Some(config::Format::Toml)).unwrap();
                config.append(parsed).unwrap();

                config.add_sink(
                    "company_api_sink",
                    &["company_api_rename"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_company_api.to_string(),
                    ),
                );
                config.add_sink(
                    "company_admin_sink",
                    &["company_admin_rename"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_company_admin.to_string(),
                    ),
                );
                config.add_sink(
                    "company_media_proxy_sink",
                    &["company_media_proxy_rename"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_company_media_proxy.to_string(),
                    ),
                );
                config.add_sink(
                    "company_unfurler_sink",
                    &["company_unfurler_filter"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_company_unfurler.to_string(),
                    ),
                );
                config.add_sink(
                    "audit_sink",
                    &["audit_rename"],
                    sinks::socket::SocketSinkConfig::make_basic_tcp_config(
                        out_addr_audit.to_string(),
                    ),
                );

                let rt = runtime();
                let (
                    output_lines_company_api,
                    output_lines_company_admin,
                    output_lines_company_media_proxy,
                    output_lines_company_unfurler,
                    output_lines_audit,
                    topology,
                ) = rt.block_on(async move {
                    let output_lines_company_api =
                        CountReceiver::receive_lines(out_addr_company_api);
                    let output_lines_company_admin =
                        CountReceiver::receive_lines(out_addr_company_admin);
                    let output_lines_company_media_proxy =
                        CountReceiver::receive_lines(out_addr_company_media_proxy);
                    let output_lines_company_unfurler =
                        CountReceiver::receive_lines(out_addr_company_unfurler);
                    let output_lines_audit = CountReceiver::receive_lines(out_addr_audit);

                    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
                    wait_for_tcp(in_addr).await;
                    (
                        output_lines_company_api,
                        output_lines_company_admin,
                        output_lines_company_media_proxy,
                        output_lines_company_unfurler,
                        output_lines_audit,
                        topology,
                    )
                });
                // Generate the inputs.
                let lines = [
                    r#"<118>3 2020-03-13T20:45:38.119Z my.host.com company-api 2004 ID960 - {"metadata": {"trace_id": "trace123", "guide_id": "guild123", "channel_id": "channel123", "method": "method"}}"#,
                    r#"<118>3 2020-03-13T20:45:38.119Z my.host.com company-admin 2004 ID960 - {"metadata": {"trace_id": "trace123", "guide_id": "guild123", "channel_id": "channel123", "method": "method"}}"#,
                    r#"<118>3 2020-03-13T20:45:38.119Z my.host.com company-media-proxy 2004 ID960 - {"ts": "2020-03-13T20:45:38.119Z"}"#,
                    r#"<118>3 2020-03-13T20:45:38.119Z my.host.com company-unfurler 2004 ID960 - {"ts": "2020-03-13T20:45:38.119Z", "msg": "unfurl"}"#,
                    r#"<118>3 2020-03-13T20:45:38.119Z my.host.com audit 2004 ID960 - qwerty"#,
                ].iter().cycle().take(num_lines).map(|&s| s.to_owned()).collect::<Vec<_>>();
                (
                    rt,
                    topology,
                    output_lines_company_api,
                    output_lines_company_admin,
                    output_lines_company_media_proxy,
                    output_lines_company_unfurler,
                    output_lines_audit,
                    lines,
                )
            },
            |(
                rt,
                topology,
                output_lines_company_api,
                output_lines_company_admin,
                output_lines_company_media_proxy,
                output_lines_company_unfurler,
                output_lines_audit,
                lines,
            )| {
                rt.block_on(async move {
                    send_lines(in_addr, lines).await.unwrap();

                    topology.stop().await;

                    let output_lines_company_api = output_lines_company_api.await.len();
                    let output_lines_company_admin = output_lines_company_admin.await.len();
                    let output_lines_company_media_proxy =
                        output_lines_company_media_proxy.await.len();
                    let output_lines_company_unfurler = output_lines_company_unfurler.await.len();
                    let output_lines_audit = output_lines_audit.await.len();

                    debug_assert!(output_lines_company_api > 0);
                    debug_assert!(output_lines_company_admin > 0);
                    debug_assert!(output_lines_company_media_proxy > 0);
                    debug_assert!(output_lines_company_unfurler > 0);
                    debug_assert!(output_lines_audit > 0);

                    (
                        output_lines_company_api,
                        output_lines_company_admin,
                        output_lines_company_media_proxy,
                        output_lines_company_unfurler,
                        output_lines_audit,
                    )
                });
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group!(
    name = benches;
    // encapsulates CI noise we saw in
    // https://github.com/timberio/vector/issues/5394
    config = Criterion::default().noise_threshold(0.20);
    targets = benchmark_simple_pipes, benchmark_interconnected, benchmark_transforms, benchmark_complex, benchmark_real_world_1
);
