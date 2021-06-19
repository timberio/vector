mod support;

use crate::support::{sink, sink_failing_healthcheck, source, transform, MockSourceConfig};
use futures::{future, stream, FutureExt, SinkExt, StreamExt};
use std::{
    collections::HashMap,
    iter,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::time::{sleep, Duration};
use vector::{config::Config, event::Event, test_util::start_topology, topology};

fn basic_config() -> Config {
    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_sink("out1", &["in1"], sink(10).1);
    config.build().unwrap()
}

fn basic_config_with_sink_failing_healthcheck() -> Config {
    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_sink("out1", &["in1"], sink_failing_healthcheck(10).1);
    config.build().unwrap()
}

fn into_message(event: Event) -> String {
    event
        .as_log()
        .get(&vector::config::log_schema().message_key())
        .unwrap()
        .to_string_lossy()
}

#[tokio::test]
async fn topology_shutdown_while_active() {
    let source_event_counter = Arc::new(AtomicUsize::new(0));
    let source_event_total = source_event_counter.clone();

    let (in1, rx) = vector::Pipeline::new_with_buffer(1000, vec![]);

    let source1 = MockSourceConfig::new_with_event_counter(rx, source_event_counter);
    let transform1 = transform(" transformed", 0.0);
    let (out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_transform("t1", &["in1"], transform1);
    config.add_sink("out1", &["t1"], sink1);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let pump_handle = tokio::spawn(async move {
        futures::stream::repeat(Event::from("test"))
            .map(Ok)
            .forward(in1)
            .await
    });

    // Wait until at least 100 events have been seen by the source so we know the pump is running
    // and pushing events through the pipeline.
    while source_event_total.load(Ordering::SeqCst) < 100 {
        sleep(Duration::from_millis(10)).await;
    }

    // Now shut down the RunningTopology while Events are still being processed.
    let stop_complete = tokio::spawn(async move { topology.stop().await });

    // Now that shutdown has begun we should be able to drain the Sink without blocking forever,
    // as the source should shut down and close its output channel.
    let processed_events = out1.collect::<Vec<_>>().await;
    assert_eq!(
        processed_events.len(),
        source_event_total.load(Ordering::Relaxed)
    );
    for event in processed_events {
        assert_eq!(
            event.as_log()[&vector::config::log_schema().message_key()],
            "test transformed".to_owned().into()
        );
    }

    stop_complete.await.unwrap();

    // We expect the pump to fail with an error since we shut down the source it was sending to
    // while it was running.
    assert!(pump_handle.await.unwrap().is_err());
}

#[tokio::test]
async fn topology_source_and_sink() {
    let (mut in1, source1) = source();
    let (out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_sink("out1", &["in1"], sink1);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let event = Event::from("this");
    in1.send(event.clone()).await.unwrap();

    topology.stop().await;

    let res = out1.collect::<Vec<_>>().await;

    assert_eq!(vec![event], res);
}

#[tokio::test]
async fn topology_multiple_sources() {
    let (mut in1, source1) = source();
    let (mut in2, source2) = source();
    let (mut out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_source("in2", source2, None, None);
    config.add_sink("out1", &["in1", "in2"], sink1);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let event1 = Event::from("this");
    let event2 = Event::from("that");

    in1.send(event1.clone()).await.unwrap();

    let out_event1 = out1.next().await;

    in2.send(event2.clone()).await.unwrap();

    let out_event2 = out1.next().await;

    topology.stop().await;

    assert_eq!(out_event1, Some(event1));
    assert_eq!(out_event2, Some(event2));
}

#[tokio::test]
async fn topology_multiple_sinks() {
    let (mut in1, source1) = source();
    let (out1, sink1) = sink(10);
    let (out2, sink2) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_sink("out1", &["in1"], sink1);
    config.add_sink("out2", &["in1"], sink2);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let event = Event::from("this");

    in1.send(event.clone()).await.unwrap();

    topology.stop().await;

    let res1 = out1.collect::<Vec<_>>().await;
    let res2 = out2.collect::<Vec<_>>().await;

    assert_eq!(vec![event.clone()], res1);
    assert_eq!(vec![event], res2);
}

#[tokio::test]
async fn topology_transform_chain() {
    let (mut in1, source1) = source();
    let transform1 = transform(" first", 0.0);
    let transform2 = transform(" second", 0.0);
    let (out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_transform("t1", &["in1"], transform1);
    config.add_transform("t2", &["t1"], transform2);
    config.add_sink("out1", &["t2"], sink1);

    let (topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let event = Event::from("this");

    in1.send(event).await.unwrap();

    topology.stop().await;

    let res = out1.map(into_message).collect::<Vec<_>>().await;

    assert_eq!(vec!["this first second"], res);
}

#[tokio::test]
async fn topology_remove_one_source() {
    let (mut in1, source1) = source();
    let (mut in2, source2) = source();
    let (_out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_source("in2", source2, None, None);
    config.add_sink("out1", &["in1", "in2"], sink1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let (out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_sink("out1", &["in1"], sink1);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());

    let event1 = Event::from("this");
    let event2 = Event::from("that");
    let h_out1 = tokio::spawn(out1.collect::<Vec<_>>());
    in1.send(event1.clone()).await.unwrap();
    in2.send(event2.clone()).await.unwrap_err();
    topology.stop().await;

    let res = h_out1.await.unwrap();
    assert_eq!(vec![event1], res);
}

#[tokio::test]
async fn topology_remove_one_sink() {
    let (mut in1, source1) = source();
    let (out1, sink1) = sink(10);
    let (out2, sink2) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_sink("out1", &["in1"], sink1);
    config.add_sink("out2", &["in1"], sink2);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_sink("out1", &["in1"], sink(10).1);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());

    let event = Event::from("this");

    in1.send(event.clone()).await.unwrap();

    topology.stop().await;

    let res1 = out1.collect::<Vec<_>>().await;
    let res2 = out2.collect::<Vec<_>>().await;

    assert_eq!(vec![event], res1);
    assert_eq!(Vec::<Event>::new(), res2);
}

#[tokio::test]
async fn topology_remove_one_transform() {
    let (mut in1, source1) = source();
    let transform1 = transform(" transformed", 0.0);
    let transform2 = transform(" transformed", 0.0);
    let (out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_transform("t1", &["in1"], transform1);
    config.add_transform("t2", &["t1"], transform2);
    config.add_sink("out1", &["t2"], sink1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let transform2 = transform(" transformed", 0.0);

    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_transform("t2", &["in1"], transform2);
    config.add_sink("out1", &["t2"], sink(10).1);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());

    let event = Event::from("this");
    let h_out1 = tokio::spawn(out1.map(into_message).collect::<Vec<_>>());
    in1.send(event.clone()).await.unwrap();
    topology.stop().await;
    let res = h_out1.await.unwrap();
    assert_eq!(vec!["this transformed"], res);
}

#[tokio::test]
async fn topology_swap_source() {
    let (mut in1, source1) = source();
    let (out1v1, sink1v1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_sink("out1", &["in1"], sink1v1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let (mut in2, source2) = source();
    let (out1v2, sink1v2) = sink(10);

    let mut config = Config::builder();
    config.add_source("in2", source2, None, None);
    config.add_sink("out1", &["in2"], sink1v2);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());

    let event1 = Event::from("this");
    let event2 = Event::from("that");

    let h_out1v1 = tokio::spawn(out1v1.collect::<Vec<_>>());
    let h_out1v2 = tokio::spawn(out1v2.collect::<Vec<_>>());
    in1.send(event1.clone()).await.unwrap_err();
    in2.send(event2.clone()).await.unwrap();
    topology.stop().await;
    let res1v1 = h_out1v1.await.unwrap();
    let res1v2 = h_out1v2.await.unwrap();

    assert_eq!(Vec::<Event>::new(), res1v1);
    assert_eq!(vec![event2], res1v2);
}

#[tokio::test]
async fn topology_swap_sink() {
    let (mut in1, source1) = source();
    let (out1, sink1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_sink("out1", &["in1"], sink1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let (out2, sink2) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_sink("out2", &["in1"], sink2);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());

    let event = Event::from("this");
    let h_out1 = tokio::spawn(out1.collect::<Vec<_>>());
    let h_out2 = tokio::spawn(out2.collect::<Vec<_>>());
    in1.send(event.clone()).await.unwrap();
    topology.stop().await;

    let res1 = h_out1.await.unwrap();
    let res2 = h_out2.await.unwrap();

    assert_eq!(Vec::<Event>::new(), res1);
    assert_eq!(vec![event], res2);
}

#[tokio::test]
async fn topology_swap_transform() {
    let (mut in1, source1) = source();
    let transform1 = transform(" transformed", 0.0);
    let (out1v1, sink1v1) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_transform("t1", &["in1"], transform1);
    config.add_sink("out1", &["t1"], sink1v1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let transform2 = transform(" replaced", 0.0);
    let (out1v2, sink1v2) = sink(10);

    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_transform("t2", &["in1"], transform2);
    config.add_sink("out1", &["t2"], sink1v2);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());

    let event = Event::from("this");
    let h_out1v1 = tokio::spawn(out1v1.map(into_message).collect::<Vec<_>>());
    let h_out1v2 = tokio::spawn(out1v2.map(into_message).collect::<Vec<_>>());
    in1.send(event.clone()).await.unwrap();
    topology.stop().await;
    let res1v1 = h_out1v1.await.unwrap();
    let res1v2 = h_out1v2.await.unwrap();

    assert_eq!(Vec::<String>::new(), res1v1);
    assert_eq!(vec!["this replaced"], res1v2);
}

#[ignore] // TODO: issue #2186
#[tokio::test]
async fn topology_swap_transform_is_atomic() {
    let (in1, source1) = source();
    let transform1v1 = transform(" transformed", 0.0);
    let (out1, sink1) = sink(10);

    let running = Arc::new(AtomicBool::new(true));
    let run_control = running.clone();

    let send_counter = Arc::new(AtomicUsize::new(0));
    let recv_counter = Arc::new(AtomicUsize::new(0));
    let send_total = send_counter.clone();
    let recv_total = recv_counter.clone();

    let events = move || {
        if running.load(Ordering::Acquire) {
            send_counter.fetch_add(1, Ordering::Release);
            Some(Event::from("this"))
        } else {
            None
        }
    };
    let input = stream::iter(iter::from_fn(events));
    let input = input
        .map(Ok)
        .forward(in1.sink_map_err(|e| panic!("{:?}", e)))
        .map(|_| ());
    let output = out1.for_each(move |_| {
        recv_counter.fetch_add(1, Ordering::Release);
        future::ready(())
    });

    let h_out = tokio::spawn(output);
    let h_in = tokio::spawn(input);

    let mut config = Config::builder();
    config.add_source("in1", source1, None, None);
    config.add_transform("t1", &["in1"], transform1v1);
    config.add_sink("out1", &["t1"], sink1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;
    sleep(Duration::from_millis(10)).await;

    let transform1v2 = transform(" replaced", 0.0);

    let mut config = Config::builder();
    config.add_source("in1", source().1, None, None);
    config.add_transform("t1", &["in1"], transform1v2);
    config.add_sink("out1", &["t1"], sink(10).1);

    assert!(topology
        .reload_config_and_respawn(config.build().unwrap())
        .await
        .unwrap());
    sleep(Duration::from_millis(10)).await;

    run_control.store(false, Ordering::Release);
    h_in.await.unwrap();
    topology.stop().await;
    h_out.await.unwrap();

    assert_eq!(
        send_total.load(Ordering::Acquire),
        recv_total.load(Ordering::Acquire)
    );
}

#[tokio::test]
async fn topology_required_healthcheck_fails_start() {
    let mut config = basic_config_with_sink_failing_healthcheck();
    config.healthchecks.require_healthy = true;
    let diff = vector::config::ConfigDiff::initial(&config);
    let pieces = topology::build_or_log_errors(&config, &diff, HashMap::new())
        .await
        .unwrap();

    assert!(topology::start_validated(config, diff, pieces)
        .await
        .is_none());
}

#[tokio::test]
async fn topology_optional_healthcheck_does_not_fail_start() {
    let config = basic_config_with_sink_failing_healthcheck();
    let diff = vector::config::ConfigDiff::initial(&config);
    let pieces = topology::build_or_log_errors(&config, &diff, HashMap::new())
        .await
        .unwrap();
    assert!(topology::start_validated(config, diff, pieces)
        .await
        .is_some());
}

#[tokio::test]
async fn topology_optional_healthcheck_does_not_fail_reload() {
    let config = basic_config();
    let (mut topology, _crash) = start_topology(config, false).await;
    let config = basic_config_with_sink_failing_healthcheck();
    assert!(topology.reload_config_and_respawn(config).await.unwrap());
}

#[tokio::test]
async fn topology_healthcheck_not_run_on_unchanged_reload() {
    let config = basic_config();

    let (mut topology, _crash) = start_topology(config, false).await;
    let mut config = basic_config_with_sink_failing_healthcheck();
    config.healthchecks.require_healthy = true;
    assert!(topology.reload_config_and_respawn(config).await.unwrap());
}

#[tokio::test]
async fn topology_healthcheck_run_for_changes_on_reload() {
    let mut config = Config::builder();
    // We can't just drop the sender side since that will close the source.
    let (_ch0, src) = source();
    config.add_source("in1", src, None, None);
    config.add_sink("out1", &["in1"], sink(10).1);

    let (mut topology, _crash) = start_topology(config.build().unwrap(), false).await;

    let mut config = Config::builder();
    // We can't just drop the sender side since that will close the source.
    let (_ch1, src) = source();
    config.add_source("in1", src, None, None);
    config.add_sink("out2", &["in1"], sink_failing_healthcheck(10).1);

    let mut config = config.build().unwrap();
    config.healthchecks.require_healthy = true;
    assert!(!topology.reload_config_and_respawn(config).await.unwrap());
}
