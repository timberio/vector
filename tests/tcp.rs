#![cfg(all(
    feature = "sinks-socket",
    feature = "transforms-sample",
    feature = "sources-socket",
))]

use approx::assert_relative_eq;
use vector::{
    config, sinks, sources,
    test_util::{
        next_addr, random_lines, send_lines, start_topology, trace_init, wait_for_tcp,
        CountReceiver,
    },
    transforms,
};

#[tokio::test]
async fn pipe() {
    let num_lines: usize = 10000;

    let in_addr = next_addr();
    let out_addr = next_addr();

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
        sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr.to_string()),
    );

    let mut output_lines = CountReceiver::receive_lines(out_addr);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr).await;

    // Wait for output to connect
    output_lines.connected().await;

    let input_lines = random_lines(100).take(num_lines).collect::<Vec<_>>();
    send_lines(in_addr, input_lines.clone()).await.unwrap();

    // Shut down server
    topology.stop().await;

    let output_lines = output_lines.await;
    assert_eq!(num_lines, output_lines.len());
    assert_eq!(input_lines, output_lines);
}

#[tokio::test]
async fn sample() {
    let num_lines: usize = 10000;

    let in_addr = next_addr();
    let out_addr = next_addr();

    let mut config = config::Config::builder();
    config.add_source(
        "in",
        sources::socket::SocketConfig::make_basic_tcp_config(in_addr),
        None,
        None,
    );
    config.add_transform(
        "sample",
        &["in"],
        transforms::sample::SampleConfig {
            rate: 10,
            key_field: Some(config::log_schema().message_key().into()),
            exclude: None,
        },
    );
    config.add_sink(
        "out",
        &["sample"],
        sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr.to_string()),
    );

    let mut output_lines = CountReceiver::receive_lines(out_addr);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr).await;

    // Wait for output to connect
    output_lines.connected().await;

    let input_lines = random_lines(100).take(num_lines).collect::<Vec<_>>();
    send_lines(in_addr, input_lines.clone()).await.unwrap();

    // Shut down server
    topology.stop().await;

    let output_lines = output_lines.await;
    let num_output_lines = output_lines.len();

    let output_lines_ratio = num_output_lines as f32 / num_lines as f32;
    assert_relative_eq!(output_lines_ratio, 0.1, epsilon = 0.01);

    let mut input_lines = input_lines.into_iter();
    // Assert that all of the output lines were present in the input and in the same order
    for output_line in output_lines {
        let next_line = input_lines.by_ref().find(|l| l == &output_line);
        assert_eq!(Some(output_line), next_line);
    }
}

#[tokio::test]
async fn fork() {
    let num_lines: usize = 10000;

    let in_addr = next_addr();
    let out_addr1 = next_addr();
    let out_addr2 = next_addr();

    let mut config = config::Config::builder();
    config.add_source(
        "in",
        sources::socket::SocketConfig::make_basic_tcp_config(in_addr),
        None,
        None,
    );
    config.add_sink(
        "out1",
        &["in"],
        sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr1.to_string()),
    );
    config.add_sink(
        "out2",
        &["in"],
        sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr2.to_string()),
    );

    let mut output_lines1 = CountReceiver::receive_lines(out_addr1);
    let mut output_lines2 = CountReceiver::receive_lines(out_addr2);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr).await;

    // Wait for output to connect
    output_lines1.connected().await;
    output_lines2.connected().await;

    let input_lines = random_lines(100).take(num_lines).collect::<Vec<_>>();
    send_lines(in_addr, input_lines.clone()).await.unwrap();

    // Shut down server
    topology.stop().await;

    let output_lines1 = output_lines1.await;
    let output_lines2 = output_lines2.await;
    assert_eq!(num_lines, output_lines1.len());
    assert_eq!(num_lines, output_lines2.len());
    assert_eq!(input_lines, output_lines1);
    assert_eq!(input_lines, output_lines2);
}

// In cpu constrained environments at least two threads
// are needed to finish processing all the events before
// sources are forcefully shutted down.
// Although that's still not a guarantee.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merge_and_fork() {
    trace_init();

    let num_lines: usize = 10000;

    let in_addr1 = next_addr();
    let in_addr2 = next_addr();
    let out_addr1 = next_addr();
    let out_addr2 = next_addr();

    // out1 receives both in1 and in2
    // out2 receives in2 only
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
        &["in2"],
        sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr2.to_string()),
    );

    let mut output_lines1 = CountReceiver::receive_lines(out_addr1);
    let mut output_lines2 = CountReceiver::receive_lines(out_addr2);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr1).await;
    wait_for_tcp(in_addr2).await;

    // Wait for output to connect
    output_lines1.connected().await;
    output_lines2.connected().await;

    let input_lines1 = random_lines(100).take(num_lines).collect::<Vec<_>>();
    let input_lines2 = random_lines(100).take(num_lines).collect::<Vec<_>>();
    send_lines(in_addr1, input_lines1.clone()).await.unwrap();
    send_lines(in_addr2, input_lines2.clone()).await.unwrap();

    // Accept connection in Vector, before shutdown
    tokio::task::yield_now().await;

    // Shut down server
    topology.stop().await;

    let output_lines1 = output_lines1.await;
    let output_lines2 = output_lines2.await;

    assert_eq!(input_lines1.len() + input_lines2.len(), output_lines1.len());
    assert_eq!(input_lines2.len(), output_lines2.len());
    assert_eq!(input_lines2, output_lines2);

    // Assert that all of the output lines were present in the input and in the same order
    let mut input_lines1 = input_lines1.into_iter().peekable();
    let mut input_lines2 = input_lines2.into_iter().peekable();
    for output_line in &output_lines1 {
        if Some(output_line) == input_lines1.peek() {
            input_lines1.next();
        } else if Some(output_line) == input_lines2.peek() {
            input_lines2.next();
        } else {
            panic!("Got line in output that wasn't in input");
        }
    }
    assert_eq!(input_lines1.next(), None);
    assert_eq!(input_lines2.next(), None);
}

#[tokio::test]
async fn reconnect() {
    let num_lines: usize = 1000;

    let in_addr = next_addr();
    let out_addr = next_addr();

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
        sinks::socket::SocketSinkConfig::make_basic_tcp_config(out_addr.to_string()),
    );

    let output_lines = CountReceiver::receive_lines(out_addr);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;
    // Wait for server to accept traffic
    wait_for_tcp(in_addr).await;

    let input_lines = random_lines(100).take(num_lines).collect::<Vec<_>>();
    send_lines(in_addr, input_lines.clone()).await.unwrap();

    // Shut down server and wait for it to fully flush
    topology.stop().await;

    let output_lines = output_lines.await;
    assert!(num_lines >= 2);
    assert!(output_lines.iter().all(|line| input_lines.contains(line)))
}
