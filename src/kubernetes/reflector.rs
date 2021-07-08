//! Watch and cache the remote Kubernetes API resources.

use super::{
    resource_version, state,
    watcher::{self, Watcher},
};
use crate::internal_events::kubernetes::reflector as internal_events;
use futures::{pin_mut, stream::StreamExt};
use k8s_openapi::{
    apimachinery::pkg::apis::meta::v1::{ObjectMeta, WatchEvent},
    Metadata, WatchOptional,
};
use snafu::Snafu;
use std::convert::Infallible;
use std::time::Duration;
use tokio::{select, time::sleep};

/// Watches remote Kubernetes resources and maintains a local representation of
/// the remote state. "Reflects" the remote state locally.
///
/// Does not expose evented API, but keeps track of the resource versions and
/// will automatically resume on desync.
pub struct Reflector<W, S>
where
    W: Watcher,
    <W as Watcher>::Object: Metadata<Ty = ObjectMeta> + Send,
    S: state::MaintainedWrite<Item = <W as Watcher>::Object>,
{
    watcher: W,
    state_writer: S,
    field_selector: Option<String>,
    label_selector: Option<String>,
    resource_version: resource_version::State,
    pause_between_requests: Duration,
}

impl<W, S> Reflector<W, S>
where
    W: Watcher,
    <W as Watcher>::Object: Metadata<Ty = ObjectMeta> + Send,
    S: state::MaintainedWrite<Item = <W as Watcher>::Object>,
{
    /// Create a new [`Reflector`].
    pub fn new(
        watcher: W,
        state_writer: S,
        field_selector: Option<String>,
        label_selector: Option<String>,
        pause_between_requests: Duration,
    ) -> Self {
        let resource_version = resource_version::State::new();
        Self {
            watcher,
            state_writer,
            field_selector,
            label_selector,
            resource_version,
            pause_between_requests,
        }
    }
}

impl<W, S> Reflector<W, S>
where
    W: Watcher,
    <W as Watcher>::Object: Metadata<Ty = ObjectMeta> + Send + Unpin + std::fmt::Debug,
    <W as Watcher>::InvocationError: Unpin,
    <W as Watcher>::StreamError: Unpin,
    S: state::MaintainedWrite<Item = <W as Watcher>::Object>,
{
    /// Run the watch loop and drive the state updates via `state_writer`.
    pub async fn run(
        &mut self,
    ) -> Result<Infallible, Error<<W as Watcher>::InvocationError, <W as Watcher>::StreamError>>
    {
        // Start the watch loop.
        'outer: loop {
            // For the next pause duration we won't get any updates.
            // This is better than flooding k8s api server with requests.
            sleep(self.pause_between_requests).await;

            let invocation_result = self.issue_request().await;
            let stream = match invocation_result {
                Ok(val) => val,
                Err(watcher::invocation::Error::Desync { source }) => {
                    emit!(internal_events::InvocationDesyncReceived { error: source });
                    // We got desynced, reset the state and retry fetching.
                    self.resource_version.reset();
                    self.state_writer.resync().await;
                    continue;
                }
                Err(watcher::invocation::Error::Recoverable { source }) => {
                    emit!(internal_events::InvocationHttpErrorReceived { error: source });
                    continue;
                }
                Err(watcher::invocation::Error::Other { source }) => {
                    // Unrecoverable error
                    // TODO: retry these errors
                    // https://github.com/timberio/vector/issues/7149
                    error!(message = "Watcher error.", error = ?source);
                    return Err(Error::Invocation { source });
                }
            };

            pin_mut!(stream);
            loop {
                // Obtain a value from the watch stream.
                // If maintenance is requested, we perform it concurrently
                // to reading items from the watch stream.
                let maintenance_request = self.state_writer.maintenance_request();
                let val = select! {
                    // If we got a maintenance request - perform the
                    // maintenance.
                    _ = async { maintenance_request.unwrap().await }, if maintenance_request.is_some() => {
                        self.state_writer.perform_maintenance().await;
                        continue;
                    }
                    // If we got a value from the watch stream - just pass it
                    // outside.
                    val = stream.next() => val,
                };
                trace!(message = "Got an item from watch stream.");

                if let Some(item) = val {
                    let response = match item {
                        // We got a stream-level desync error, abort the stream
                        // and handle the desync.
                        Err(watcher::stream::Error::Desync { source }) => {
                            emit!(internal_events::StreamDesyncReceived { error: source });
                            // We got desynced, reset the state and retry fetching.
                            self.resource_version.reset();
                            self.state_writer.resync().await;
                            continue 'outer;
                        }
                        // We have an error, attempt to recover from this error and continue the outer loop.
                        Err(watcher::stream::Error::Recoverable { source }) => {
                            emit!(internal_events::InvocationHttpErrorReceived { error: source });
                            continue 'outer;
                        }
                        // A fine watch respose arrived, we just pass it down.
                        Ok(val) => val,
                    };
                    self.process_watch_event(response).await;
                } else {
                    // Response stream has ended.
                    // Break the watch reading loop so the flow can
                    // continue an issue a new watch request.
                    break;
                }
            }
        }
    }

    /// Prepare and execute a watch request.
    async fn issue_request(
        &mut self,
    ) -> Result<<W as Watcher>::Stream, watcher::invocation::Error<<W as Watcher>::InvocationError>>
    {
        let watch_optional = WatchOptional {
            field_selector: self.field_selector.as_deref(),
            label_selector: self.label_selector.as_deref(),
            pretty: None,
            resource_version: self.resource_version.get(),
            timeout_seconds: Some(290), // https://github.com/kubernetes/kubernetes/issues/6513
            allow_watch_bookmarks: Some(true),
        };
        let stream = self.watcher.watch(watch_optional).await?;
        Ok(stream)
    }

    /// Process an item from the watch response stream.
    async fn process_watch_event(&mut self, event: WatchEvent<<W as Watcher>::Object>) {
        // Prepare a resource version candidate so we can update (aka commit) it
        // later.
        let resource_version_candidate = match resource_version::Candidate::from_watch_event(&event)
        {
            Some(val) => val,
            None => {
                // This event doesn't have a resource version, this means
                // it's not something we care about.
                return;
            }
        };

        // Process the event.
        self.process_event(event).await;

        // Record the resourse version for this event, so when we resume
        // it won't be redelivered.
        self.resource_version.update(resource_version_candidate);
    }

    /// Translate received watch event to the state update.
    async fn process_event(&mut self, event: WatchEvent<<W as Watcher>::Object>) {
        println!("{:?}", &event);
        match event {
            WatchEvent::Added(object) => {
                trace!(message = "Got an object event.", event = "added");
                self.state_writer.add(object).await;
            }
            WatchEvent::Deleted(object) => {
                trace!(message = "Got an object event.", event = "deleted");
                self.state_writer.delete(object).await;
            }
            WatchEvent::Modified(object) => {
                trace!(message = "Got an object event.", event = "modified");
                self.state_writer.update(object).await;
            }
            WatchEvent::Bookmark { .. } => {
                trace!(message = "Got an object event.", event = "bookmark");
                // noop
            }
            _ => unreachable!("Other event types should never reach this code."),
        }
    }
}

/// Errors that can occur while watching.
#[derive(Debug, Snafu)]
pub enum Error<I, S>
where
    I: std::error::Error + 'static,
    S: std::error::Error + 'static,
{
    /// Returned when the watch invocation (HTTP request) failed.
    #[snafu(display("watch invocation failed"))]
    Invocation {
        /// The underlying invocation error.
        source: I,
    },

    /// Returned when the stream failed with an error.
    #[snafu(display("streaming error"))]
    Streaming {
        /// The underlying stream error.
        source: S,
    },
}

#[cfg(test)]
mod tests {
    use super::{Error, Reflector};
    use crate::{
        kubernetes::{
            instrumenting_watcher::InstrumentingWatcher,
            mock_watcher::{self, MockWatcher},
            state,
        },
        test_util::trace_init,
    };
    use futures::{channel::mpsc, SinkExt, StreamExt};
    use k8s_openapi::{
        api::core::v1::Pod,
        apimachinery::pkg::apis::meta::v1::{ObjectMeta, WatchEvent},
        Metadata,
    };
    use std::time::Duration;

    /// A helper function to simplify assertion on the `evmap` state.
    fn gather_state<T>(handle: &evmap::ReadHandle<String, state::evmap::Value<T>>) -> Vec<T>
    where
        T: Metadata<Ty = ObjectMeta> + Clone,
    {
        let mut vec: Vec<(String, T)> = handle
            .read()
            .expect("expected read to be ready")
            .iter()
            .map(|(key, values)| {
                assert_eq!(values.len(), 1);
                let value = values.get_one().unwrap();
                (key.clone(), value.as_ref().as_ref().to_owned())
            })
            .collect();

        // Sort the results by key for consistent assertions.
        vec.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        // Discard keys.
        vec.into_iter().map(|(_, value)| value).collect()
    }

    // A helper to build a pod object for test purposes.
    fn make_pod(uid: &str, resource_version: &str) -> Pod {
        Pod {
            metadata: ObjectMeta {
                uid: Some(uid.to_owned()),
                resource_version: Some(resource_version.to_owned()),
                ..ObjectMeta::default()
            },
            ..Pod::default()
        }
    }

    // A type alias to add expressiveness.
    type StateSnapshot = Vec<Pod>;

    // A helper enum to encode expected mock watcher invocation.
    enum ExpInvRes {
        Stream(Vec<ExpStmRes>),
        Desync,
        Recoverable,
    }

    // A helper enum to encode expected mock watcher stream.
    enum ExpStmRes {
        Item(WatchEvent<Pod>),
        Desync,
    }

    // A simple test, to serve as a bare-bones example for adding further tests.
    #[tokio::test]
    async fn simple_test() {
        trace_init();

        // Prepare state.
        let (state_events_tx, _state_events_rx) = mpsc::channel(0);
        let (_state_actions_tx, state_actions_rx) = mpsc::channel(0);
        let state_writer = state::mock::Writer::new(state_events_tx, state_actions_rx);
        let state_writer = state::instrumenting::Writer::new(state_writer);

        // Prepare watcher.
        let (watcher_events_tx, mut watcher_events_rx) = mpsc::channel(0);
        let (mut watcher_invocations_tx, watcher_invocations_rx) = mpsc::channel(0);
        let watcher = MockWatcher::<Pod>::new(watcher_events_tx, watcher_invocations_rx);
        let watcher = InstrumentingWatcher::new(watcher);

        // Prepare reflector.
        let mut reflector =
            Reflector::new(watcher, state_writer, None, None, Duration::from_secs(1));

        // Run test logic.
        let logic = tokio::spawn(async move {
            // Wait for watcher to request next invocation.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));

            // We're done with the test, send the error to terminate the
            // reflector.
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::ErrOther)
                .await
                .unwrap();
        });

        // Run the test and wait for an error.
        let result = reflector.run().await;

        // Join on the logic first, to report logic errors with higher
        // priority.
        logic.await.unwrap();

        // The only way reflector completes is with an error, but that's ok.
        // In tests we make it exit with an error to complete the test.
        result.unwrap_err();

        // Explicitly drop the reflector at the very end.
        drop(reflector);
    }

    // Test the properties of the normal  execution flow.
    #[tokio::test]
    async fn flow_test() {
        trace_init();

        let invocations = vec![
            (
                vec![],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid0", "10"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid1", "15"))),
                ]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Modified(make_pod("uid0", "20"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid2", "25"))),
                ]),
            ),
            (
                vec![
                    make_pod("uid0", "20"),
                    make_pod("uid1", "15"),
                    make_pod("uid2", "25"),
                ],
                Some("25".to_owned()),
                ExpInvRes::Stream(vec![ExpStmRes::Item(WatchEvent::Bookmark {
                    resource_version: "50".into(),
                })]),
            ),
            (
                vec![
                    make_pod("uid0", "20"),
                    make_pod("uid1", "15"),
                    make_pod("uid2", "25"),
                ],
                Some("50".to_owned()),
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Deleted(make_pod("uid2", "55"))),
                    ExpStmRes::Item(WatchEvent::Modified(make_pod("uid0", "60"))),
                ]),
            ),
        ];
        let expected_resulting_state = vec![make_pod("uid0", "60"), make_pod("uid1", "15")];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    // Test the properies of the flow with desync during invocation.
    #[tokio::test]
    async fn invocation_desync_test() {
        trace_init();

        let invocations = vec![
            (
                vec![],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid0", "10"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid1", "15"))),
                ]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Desync,
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid20", "1000"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid21", "1005"))),
                ]),
            ),
            (
                vec![make_pod("uid20", "1000"), make_pod("uid21", "1005")],
                Some("1005".to_owned()),
                ExpInvRes::Stream(vec![ExpStmRes::Item(WatchEvent::Modified(make_pod(
                    "uid21", "1010",
                )))]),
            ),
        ];
        let expected_resulting_state = vec![make_pod("uid20", "1000"), make_pod("uid21", "1010")];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    // Test
    #[tokio::test]
    async fn invocation_recoverable_test() {
        trace_init();

        let invocations = vec![
            (
                vec![],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid0", "10"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid1", "15"))),
                ]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Recoverable,
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid20", "1000"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid21", "1005"))),
                ]),
            ),
        ];
        let expected_resulting_state = vec![
            make_pod("uid0", "10"),
            make_pod("uid1", "15"),
            make_pod("uid20", "1000"),
            make_pod("uid21", "1005"),
        ];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    // Test
    #[tokio::test]
    async fn consecutive_invocation_recoverable_test() {
        trace_init();

        let invocations = vec![
            (
                vec![],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid0", "10"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid1", "15"))),
                ]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Recoverable,
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Recoverable,
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid20", "1000"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid21", "1005"))),
                ]),
            ),
        ];
        let expected_resulting_state = vec![
            make_pod("uid0", "10"),
            make_pod("uid1", "15"),
            make_pod("uid20", "1000"),
            make_pod("uid21", "1005"),
        ];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    // Test the properies of the flow with desync during stream when bare desync arrives.
    #[tokio::test]
    async fn stream_desync_test_bare() {
        trace_init();

        let invocations = vec![
            (
                vec![],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid0", "10"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid1", "15"))),
                ]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Stream(vec![ExpStmRes::Desync]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid20", "1000"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid21", "1005"))),
                ]),
            ),
            (
                vec![make_pod("uid20", "1000"), make_pod("uid21", "1005")],
                Some("1005".to_owned()),
                ExpInvRes::Stream(vec![ExpStmRes::Item(WatchEvent::Modified(make_pod(
                    "uid21", "1010",
                )))]),
            ),
        ];
        let expected_resulting_state = vec![make_pod("uid20", "1000"), make_pod("uid21", "1010")];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    // Test the properies of the flow with desync during stream when desync arrives after an item.
    #[tokio::test]
    async fn stream_desync_test_with_item() {
        trace_init();

        let invocations = vec![
            (
                vec![],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid0", "10"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid1", "15"))),
                ]),
            ),
            (
                vec![make_pod("uid0", "10"), make_pod("uid1", "15")],
                Some("15".to_owned()),
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid2", "20"))),
                    ExpStmRes::Desync,
                ]),
            ),
            (
                vec![
                    make_pod("uid0", "10"),
                    make_pod("uid1", "15"),
                    make_pod("uid2", "20"),
                ],
                None,
                ExpInvRes::Stream(vec![
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid20", "1000"))),
                    ExpStmRes::Item(WatchEvent::Added(make_pod("uid21", "1005"))),
                ]),
            ),
            (
                vec![make_pod("uid20", "1000"), make_pod("uid21", "1005")],
                Some("1005".to_owned()),
                ExpInvRes::Stream(vec![ExpStmRes::Item(WatchEvent::Modified(make_pod(
                    "uid21", "1010",
                )))]),
            ),
        ];
        let expected_resulting_state = vec![make_pod("uid20", "1000"), make_pod("uid21", "1010")];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    /// Test that the state is properly initialized even if no events arrived.
    #[tokio::test]
    async fn no_updates_state_test() {
        trace_init();

        let invocations = vec![];
        let expected_resulting_state = vec![];

        // Use standard flow test logic.
        run_flow_test(invocations, expected_resulting_state).await;
    }

    // Test that [`k8s_openapi::WatchOptional`] is populated properly.
    #[tokio::test]
    async fn arguments_test() {
        trace_init();

        // Prepare state.
        let (state_events_tx, _state_events_rx) = mpsc::channel(0);
        let (_state_actions_tx, state_actions_rx) = mpsc::channel(0);
        let state_writer = state::mock::Writer::new(state_events_tx, state_actions_rx);

        // Prepare watcher.
        let (watcher_events_tx, mut watcher_events_rx) = mpsc::channel(0);
        let (mut watcher_invocations_tx, watcher_invocations_rx) = mpsc::channel(0);
        let watcher = MockWatcher::<Pod>::new(watcher_events_tx, watcher_invocations_rx);
        let watcher = InstrumentingWatcher::new(watcher);

        // Prepare reflector.
        let mut reflector = Reflector::new(
            watcher,
            state_writer,
            Some("fields".to_owned()),
            Some("labels".to_owned()),
            Duration::from_secs(1),
        );

        // Run test logic.
        let logic = tokio::spawn(async move {
            // Wait for watcher to request next invocation.
            let invocation_event = watcher_events_rx.next().await.unwrap();

            // Assert that we obtained an invocation event and obtain
            // the passed `watch_optional`.
            let watch_optional = match invocation_event {
                mock_watcher::ScenarioEvent::Invocation(val) => val,
                _ => panic!("Unexpected event from watcher mock"),
            };

            // Assert that the arguments are passed properly.
            assert_eq!(
                watch_optional,
                mock_watcher::OwnedWatchOptional {
                    allow_watch_bookmarks: Some(true),
                    field_selector: Some("fields".to_owned()),
                    label_selector: Some("labels".to_owned()),
                    pretty: None,
                    resource_version: None,
                    timeout_seconds: Some(290),
                }
            );

            // We're done with the test, send the error to terminate the
            // reflector.
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::ErrOther)
                .await
                .unwrap();
        });

        // Run the test and wait for an error.
        let result = reflector.run().await;

        // Join on the logic first, to report logic errors with higher
        // priority.
        logic.await.unwrap();

        // The only way reflector completes is with an error, but that's ok.
        // In tests we make it exit with an error to complete the test.
        result.unwrap_err();

        // Explicitly drop the reflector at the very end.
        drop(reflector);
    }

    /// Test that the delayed delete works accordingly.
    #[tokio::test]
    async fn test_delayed_deletes() {
        trace_init();

        // Freeze time.
        tokio::time::pause();

        // Prepare state.
        let (state_events_tx, mut state_events_rx) = mpsc::channel(0);
        let (mut state_actions_tx, state_actions_rx) = mpsc::channel(0);
        let state_writer = state::mock::Writer::new(state_events_tx, state_actions_rx);
        let state_writer = state::instrumenting::Writer::new(state_writer);
        let deletion_delay = Duration::from_secs(600);
        let state_writer = state::delayed_delete::Writer::new(state_writer, deletion_delay);

        // Prepare watcher.
        let (watcher_events_tx, mut watcher_events_rx) = mpsc::channel(0);
        let (mut watcher_invocations_tx, watcher_invocations_rx) = mpsc::channel(0);
        let watcher = MockWatcher::<Pod>::new(watcher_events_tx, watcher_invocations_rx);
        let watcher = InstrumentingWatcher::new(watcher);

        // Prepare reflector.
        let pause_between_requests = Duration::from_secs(1);
        let mut reflector =
            Reflector::new(watcher, state_writer, None, None, pause_between_requests);

        // Run test logic.
        let logic = tokio::spawn(async move {
            // Advance the time to scroll pass the delay between
            // the invocations.
            tokio::time::advance(pause_between_requests * 2).await;

            // Wait for watcher to request next invocation.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));

            // Provide watcher with a new stream.
            let (mut watch_stream_tx, watch_stream_rx) = mpsc::channel(0);
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::Ok(watch_stream_rx))
                .await
                .unwrap();

            // Wait for watcher to request next item from the stream.
            assert_eq!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Stream
            );

            // Send pod addition to a stream.
            watch_stream_tx
                .send(mock_watcher::ScenarioActionStream::Ok(WatchEvent::Added(
                    make_pod("uid0", "10"),
                )))
                .await
                .unwrap();

            // Let the reflector work until the pod addition propagates to
            // the state.
            assert_eq!(
                state_events_rx.next().await.unwrap().unwrap_op(),
                (make_pod("uid0", "10"), state::mock::OpKind::Add),
            );

            // Send the confirmation of the processing at the state.
            state_actions_tx.send(()).await.unwrap();

            // Let the reflector work until watcher requests next event from
            // the stream.
            assert_eq!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Stream
            );

            // Send pod deletion to a stream.
            watch_stream_tx
                .send(mock_watcher::ScenarioActionStream::Ok(WatchEvent::Deleted(
                    make_pod("uid0", "15"),
                )))
                .await
                .unwrap();

            // Let the reflector work until watcher requests next event from
            // the stream.
            assert_eq!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Stream
            );

            // Assert that the state didn't get the deletion (yet).
            // State completes before the next item is requested from the
            // watch stream, and since we waited for the stream item to
            // be requested - we're guaranteed to have no race condition
            // here.
            assert!(state_events_rx.try_next().is_err());

            // Advance the time 10 times the deletion delay.
            tokio::time::advance(deletion_delay * 10).await;

            // At this point, maintenance should be performed, for both
            // delayed deletion state and mock state.

            // Delayed deletes are processed first.
            assert_eq!(
                state_events_rx.next().await.unwrap().unwrap_op(),
                (make_pod("uid0", "15"), state::mock::OpKind::Delete),
            );

            // Send the confirmation of the processing at the state.
            state_actions_tx.send(()).await.unwrap();

            // Then, the maintenance event should be triggered.
            // This completes the `perform_maintenance` call.
            assert!(matches!(
                state_events_rx.next().await.unwrap(),
                state::mock::ScenarioEvent::Maintenance
            ));

            // Send the confirmation of the processing at the state.
            state_actions_tx.send(()).await.unwrap();

            // We're done with the test! Shutdown the stream and force an
            // invocation error to terminate the reflector.

            // Watcher is still waiting for the item on stream.
            // Send done notification to the stream.
            watch_stream_tx
                .send(mock_watcher::ScenarioActionStream::Done)
                .await
                .unwrap();

            // Advance the time to scroll pass the delay between
            // the invocations.
            tokio::time::advance(pause_between_requests * 2).await;
            // Wait for next invocation and send an error to terminate the
            // flow.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::ErrOther)
                .await
                .unwrap();
        });

        // Run the test and wait for an error.
        let result = reflector.run().await;

        // Join on the logic first, to report logic errors with higher
        // priority.
        logic.await.unwrap();

        // The only way reflector completes is with an error, but that's ok.
        // In tests we make it exit with an error to complete the test.
        result.unwrap_err();

        // Explicitly drop the reflector at the very end.
        drop(reflector);

        // Unfreeze time.
        tokio::time::resume();
    }

    /// Test that stream error terminates the reflector.
    #[tokio::test]
    async fn test_stream_error() {
        trace_init();

        // Prepare state.
        let (state_events_tx, _state_events_rx) = mpsc::channel(0);
        let (_state_actions_tx, state_actions_rx) = mpsc::channel(0);
        let state_writer = state::mock::Writer::new(state_events_tx, state_actions_rx);
        let state_writer = state::instrumenting::Writer::new(state_writer);

        // Prepare watcher.
        let (watcher_events_tx, mut watcher_events_rx) = mpsc::channel(0);
        let (mut watcher_invocations_tx, watcher_invocations_rx) = mpsc::channel(0);
        let watcher = MockWatcher::<Pod>::new(watcher_events_tx, watcher_invocations_rx);
        let watcher = InstrumentingWatcher::new(watcher);

        // Prepare reflector.
        let mut reflector =
            Reflector::new(watcher, state_writer, None, None, Duration::from_secs(1));

        // Run test logic.
        let logic = tokio::spawn(async move {
            // Wait for watcher to request next invocation.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));

            // Provide watcher with a new stream.
            let (mut watch_stream_tx, watch_stream_rx) = mpsc::channel(0);
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::Ok(watch_stream_rx))
                .await
                .unwrap();

            // Wait for watcher to request next item from the stream.
            assert_eq!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Stream
            );

            // Send an error to the stream.
            watch_stream_tx
                .send(mock_watcher::ScenarioActionStream::ErrRecoverable)
                .await
                .unwrap();

            // Wait for watcher to request next item from the stream.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));

            // We're done with the test, send the error to terminate the
            // reflector.
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::ErrOther)
                .await
                .unwrap();
        });

        // Run the test and wait for an error.
        let result = reflector.run().await;

        // Join on the logic first, to report logic errors with higher
        // priority.
        logic.await.unwrap();

        // Assert that the reflector properly passed the error.
        assert!(matches!(result.unwrap_err(), Error::Invocation { .. }));

        // Explicitly drop the reflector at the very end.
        drop(reflector);
    }

    /// Test that maintenance works accordingly.
    #[tokio::test]
    async fn test_maintenance() {
        trace_init();

        // Prepare state.
        let (state_events_tx, mut state_events_rx) = mpsc::channel(0);
        let (mut state_actions_tx, state_actions_rx) = mpsc::channel(0);
        let (state_maintenance_request_events_tx, mut state_maintenance_request_events_rx) =
            mpsc::channel(0);
        let (mut state_maintenance_request_actions_tx, state_maintenance_request_actions_rx) =
            mpsc::channel(0);
        let state_writer = state::mock::Writer::new_with_maintenance(
            state_events_tx,
            state_actions_rx,
            state_maintenance_request_events_tx,
            state_maintenance_request_actions_rx,
        );
        let state_writer = state::instrumenting::Writer::new(state_writer);

        // Prepare watcher.
        let (watcher_events_tx, mut watcher_events_rx) = mpsc::channel(0);
        let (mut watcher_invocations_tx, watcher_invocations_rx) = mpsc::channel(0);
        let watcher = MockWatcher::<Pod>::new(watcher_events_tx, watcher_invocations_rx);
        let watcher = InstrumentingWatcher::new(watcher);

        // Prepare reflector.
        let mut reflector =
            Reflector::new(watcher, state_writer, None, None, Duration::from_secs(1));

        // Run test logic.
        let logic = tokio::spawn(async move {
            // Wait for watcher to request next invocation.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));

            // Assert that maintenance request events didn't arrive yet.
            assert!(state_maintenance_request_events_rx.try_next().is_err());

            // Provide watcher with a new stream.
            let (mut watch_stream_tx, watch_stream_rx) = mpsc::channel(0);
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::Ok(watch_stream_rx))
                .await
                .unwrap();

            // Wait for reflector to request a state maintenance.
            state_maintenance_request_events_rx.next().await.unwrap();

            // Send the maintenance request action to advance to the
            // maintenance.
            state_maintenance_request_actions_tx.send(()).await.unwrap();

            // Wait for a maintenance perform event arrival.
            assert!(matches!(
                state_events_rx.next().await.unwrap(),
                state::mock::ScenarioEvent::Maintenance
            ));

            // Send the confirmation of the state maintenance.
            state_actions_tx.send(()).await.unwrap();

            // Let the reflector work until watcher requests next event from
            // the stream.
            assert_eq!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Stream
            );

            // We're done with the test! Shutdown the stream and force an
            // invocation error to terminate the reflector.

            // Watcher is still waiting for the item on stream.
            // Send done notification to the stream.
            watch_stream_tx
                .send(mock_watcher::ScenarioActionStream::Done)
                .await
                .unwrap();

            // Wait for next invocation and send an error to terminate the
            // flow.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::ErrOther)
                .await
                .unwrap();
        });

        // Run the test and wait for an error.
        let result = reflector.run().await;

        // Join on the logic first, to report logic errors with higher
        // priority.
        logic.await.unwrap();

        // The only way reflector completes is with an error, but that's ok.
        // In tests we make it exit with an error to complete the test.
        result.unwrap_err();

        // Explicitly drop the reflector at the very end.
        drop(reflector);
    }

    // A helper function to run a flow test.
    // Use this to test various flows without the test code repetition.
    async fn run_flow_test(
        invocations: Vec<(StateSnapshot, Option<String>, ExpInvRes)>,
        expected_resulting_state: StateSnapshot,
    ) {
        // Freeze time.
        tokio::time::pause();

        // Prepare state.
        let (state_reader, state_writer) = evmap::new();
        let state_writer = state::evmap::Writer::new(state_writer, None); // test without debounce to avouid complexity
        let state_writer = state::instrumenting::Writer::new(state_writer);
        let resulting_state_reader = state_reader.clone();

        // Prepare watcher.
        let (watcher_events_tx, mut watcher_events_rx) = mpsc::channel(0);
        let (mut watcher_invocations_tx, watcher_invocations_rx) = mpsc::channel(0);
        let watcher: MockWatcher<Pod> = MockWatcher::new(watcher_events_tx, watcher_invocations_rx);
        let watcher = InstrumentingWatcher::new(watcher);

        // Prepare reflector.
        let pause_between_requests = Duration::from_secs(60 * 60); // 1 hour
        let mut reflector =
            Reflector::new(watcher, state_writer, None, None, pause_between_requests);

        // Run test logic.
        let logic = tokio::spawn(async move {
            // Process the invocations.
            'invocation: for (
                expected_state_before_op,
                expected_resource_version,
                expected_invocation_response,
            ) in invocations
            {
                // Validate that there's a delay before the invocation, and
                // that some time has to pass before the actual invocation is
                // issued.
                // Wait for a quater of the expected delay, and assert that the
                // invocation is still not yet requested.
                tokio::time::advance(pause_between_requests / 4).await;
                tokio::task::yield_now().await;
                assert!(
                    watcher_events_rx.try_next().is_err(),
                    "expected a delay before the invocation, but it was called without a delay"
                );

                // Advance the time to scroll pass the delay between
                // the invocations.
                tokio::time::advance(pause_between_requests * 2).await;

                // Wait for watcher to request next invocation.
                let invocation_event = watcher_events_rx.next().await.unwrap();

                // Assert that we obtained an invocation event.
                let watch_optional = match invocation_event {
                    mock_watcher::ScenarioEvent::Invocation(val) => val,
                    _ => panic!("Unexpected event from watcher mock"),
                };

                // Assert the current state while within the watcher stream
                // item production code.
                let state = gather_state(&state_reader);
                assert_eq!(state, expected_state_before_op);

                // Assert the resource version passed with watch invocation.
                assert_eq!(watch_optional.resource_version, expected_resource_version);

                // Determine the requested action from the test scenario.
                let actions = match expected_invocation_response {
                    // Stream is requested, continue with the current flow.
                    ExpInvRes::Stream(actions) => actions,
                    // Desync is requested, complete the invocation with the desync.
                    ExpInvRes::Desync => {
                        // Send the desync error to mock watcher.
                        watcher_invocations_tx
                            .send(mock_watcher::ScenarioActionInvocation::ErrDesync)
                            .await
                            .unwrap();
                        continue;
                    }
                    // Recoverable error is requested, complete the invocation with the error.
                    ExpInvRes::Recoverable => {
                        // Send the recoverable error to mock watcher.
                        watcher_invocations_tx
                            .send(mock_watcher::ScenarioActionInvocation::ErrRecoverable)
                            .await
                            .unwrap();
                        continue;
                    }
                };

                // Prepare channels for use in stream of the watch mock.
                let (mut watch_stream_tx, watch_stream_rx) = mpsc::channel(0);

                // Send the stream action to the watch invocation.
                watcher_invocations_tx
                    .send(mock_watcher::ScenarioActionInvocation::Ok(watch_stream_rx))
                    .await
                    .unwrap();

                for action in actions {
                    // Wait for watcher to request next item from the stream.
                    assert_eq!(
                        watcher_events_rx.next().await.unwrap(),
                        mock_watcher::ScenarioEvent::Stream
                    );

                    // Determine the requested action from the test scenario.
                    match action {
                        // Watch response is requested, send it to the stream.
                        ExpStmRes::Item(response) => {
                            // Send the requested action to the stream.
                            watch_stream_tx
                                .send(mock_watcher::ScenarioActionStream::Ok(response))
                                .await
                                .unwrap();
                        }
                        // Desync is requested, send it to the stream.
                        ExpStmRes::Desync => {
                            // Send the desync action to mock watcher.
                            watch_stream_tx
                                .send(mock_watcher::ScenarioActionStream::ErrDesync)
                                .await
                                .unwrap();
                            // After the desync, expect the reflector to go
                            // immediately to the next invocation, skipping the
                            // rest of the stream content, and not polling it
                            // anymore.
                            continue 'invocation;
                        }
                    };
                }

                // Wait for watcher to request next item from the stream.
                assert_eq!(
                    watcher_events_rx.next().await.unwrap(),
                    mock_watcher::ScenarioEvent::Stream
                );

                // Send the notification that the stream is over.
                watch_stream_tx
                    .send(mock_watcher::ScenarioActionStream::Done)
                    .await
                    .unwrap();
            }

            // We're done with the test! Shutdown the stream and force an
            // invocation error to terminate the reflector.

            // Wait for next invocation and send an error to terminate the
            // flow.
            assert!(matches!(
                watcher_events_rx.next().await.unwrap(),
                mock_watcher::ScenarioEvent::Invocation(_)
            ));
            watcher_invocations_tx
                .send(mock_watcher::ScenarioActionInvocation::ErrOther)
                .await
                .unwrap();
        });

        // Run the test and wait for an error.
        let result = reflector.run().await;

        // Join on the logic first, to report logic errors with higher
        // priority.
        logic.await.unwrap();

        // The only way reflector completes is with an error, but that's ok.
        // In tests we make it exit with an error to complete the test.
        result.unwrap_err();

        // Assert the state after the reflector exit.
        let resulting_state = gather_state(&resulting_state_reader);
        assert_eq!(resulting_state, expected_resulting_state);

        // Explicitly drop the reflector at the very end.
        // Internal evmap is dropped with the reflector, so readers won't
        // work after drop.
        drop(reflector);

        // Unfreeze time.
        tokio::time::resume();
    }
}
