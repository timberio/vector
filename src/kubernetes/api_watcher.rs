//! A watcher based on the k8s API.

use super::{
    client::Client,
    stream as k8s_stream,
    watch_request_builder::WatchRequestBuilder,
    watcher::{self, Watcher},
};
use crate::internal_events::kubernetes::api_watcher as internal_events;
use futures::{
    future::BoxFuture,
    stream::{BoxStream, Stream, StreamExt},
};
use http::StatusCode;
use hyper::Error as BodyError;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::WatchEvent;
use k8s_openapi::WatchOptional;
use snafu::{ResultExt, Snafu};

/// A simple watcher atop of the Kubernetes API [`Client`].
pub struct ApiWatcher<B>
where
    B: 'static,
{
    client: Client,
    request_builder: B,
}

impl<B> ApiWatcher<B>
where
    B: 'static,
{
    /// Create a new [`ApiWatcher`].
    pub fn new(client: Client, request_builder: B) -> Self {
        Self {
            client,
            request_builder,
        }
    }
}

impl<B> ApiWatcher<B>
where
    B: 'static + WatchRequestBuilder,
    <B as WatchRequestBuilder>::Object: Send + Unpin,
{
    async fn invoke(
        &mut self,
        watch_optional: WatchOptional<'_>,
    ) -> Result<
        impl Stream<
                Item = Result<
                    WatchEvent<<B as WatchRequestBuilder>::Object>,
                    watcher::stream::Error<stream::Error>,
                >,
            > + 'static,
        watcher::invocation::Error<invocation::Error>,
    > {
        // Prepare request.
        let request = self
            .request_builder
            .build(watch_optional)
            .context(invocation::RequestPreparation)?;
        emit!(internal_events::RequestPrepared { request: &request });

        // Send request, get response.
        let response = match self.client.send(request).await {
            Ok(response) => response,
            Err(source @ crate::http::HttpError::CallRequest { .. }) => {
                return Err(watcher::invocation::Error::recoverable(
                    invocation::Error::Request { source },
                ))
            }
            Err(source) => {
                return Err(watcher::invocation::Error::other(
                    invocation::Error::Request { source },
                ))
            }
        };

        emit!(internal_events::ResponseReceived {
            response: &response
        });

        // Handle response status code.
        let status = response.status();
        if status != StatusCode::OK {
            let source = invocation::Error::BadStatus { status };
            let err = if status == StatusCode::GONE {
                watcher::invocation::Error::desync(source)
            } else {
                watcher::invocation::Error::other(source)
            };
            return Err(err);
        }

        // Stream response body.
        let body = response.into_body();
        Ok(k8s_stream::body(body).map(|item| match item {
            Ok(WatchEvent::ErrorStatus(status)) if status.code == Some(410) => {
                Err(watcher::stream::Error::desync(stream::Error::Desync))
            }
            Ok(val) => Ok(val),
            Err(err) => Err(watcher::stream::Error::recoverable(
                stream::Error::K8sStream { source: err },
            )),
        }))
    }
}

impl<B> Watcher for ApiWatcher<B>
where
    B: 'static + WatchRequestBuilder + Send,
    <B as WatchRequestBuilder>::Object: Send + Unpin,
{
    type Object = <B as WatchRequestBuilder>::Object;

    type InvocationError = invocation::Error;

    type StreamError = stream::Error;
    type Stream = BoxStream<
        'static,
        Result<WatchEvent<Self::Object>, watcher::stream::Error<Self::StreamError>>,
    >;

    fn watch<'a>(
        &'a mut self,
        watch_optional: WatchOptional<'a>,
    ) -> BoxFuture<'a, Result<Self::Stream, watcher::invocation::Error<Self::InvocationError>>>
    {
        Box::pin(async move {
            self.invoke(watch_optional)
                .await
                .map(Box::pin)
                .map(|stream| stream as BoxStream<_>)
        })
    }
}

pub mod invocation {
    //! Invocation error.
    use super::*;

    /// Errors that can occur while watching.
    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        /// Returned when the call-specific request builder fails.
        #[snafu(display("failed to prepare an HTTP request"))]
        RequestPreparation {
            /// The underlying error.
            source: k8s_openapi::RequestError,
        },

        /// Returned when the HTTP client fails to perform an HTTP request.
        #[snafu(display("error during the HTTP request"))]
        Request {
            /// The error that API client returned.
            source: crate::http::HttpError,
        },

        /// Returned when the HTTP response has a bad status.
        #[snafu(display("HTTP response has a bad status: {}", status))]
        BadStatus {
            /// The status from the HTTP response.
            status: StatusCode,
        },
    }

    impl From<Error> for watcher::invocation::Error<Error> {
        fn from(source: Error) -> Self {
            watcher::invocation::Error::other(source)
        }
    }
}

pub mod stream {
    //! Stream error.
    use super::*;

    /// Errors that can occur while streaming the watch response.
    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        /// Returned when the stream-specific error occurs.
        #[snafu(display("k8s stream error"))]
        K8sStream {
            /// The underlying error.
            source: k8s_stream::Error<BodyError>,
        },
        /// Returned when desync watch response is detected.
        #[snafu(display("desync"))]
        Desync,
    }

    impl From<Error> for watcher::invocation::Error<Error> {
        fn from(source: Error) -> Self {
            watcher::invocation::Error::other(source)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        kubernetes::{api_watcher, client},
        tls::TlsOptions,
    };

    use super::*;
    use futures::StreamExt;
    use httpmock::MockServer;
    use httpmock::{Method::GET, Then, When};
    use k8s_openapi::WatchOptional;
    use k8s_openapi::{
        api::core::v1::Pod,
        apimachinery::pkg::apis::meta::v1::{ObjectMeta, Status, WatchEvent},
    };

    macro_rules! assert_matches {
        ($expression:expr, $($pattern:tt)+) => {
            match $expression {
                $($pattern)+ => (),
                ref e => panic!("assertion failed: `{:?}` does not match `{}`", e, stringify!($($pattern)+)),
            }
        }
    }

    /// Test that it can handle invocation errors.
    #[tokio::test]
    async fn test_invocation_errors() {
        let cases: Vec<(Box<dyn FnOnce(When, Then)>, _, _)> = vec![
            // Desync.
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(410)
                        .header("Content-Type", "application/json")
                        .body("body");
                }),
                Some(StatusCode::GONE),
                true,
            ),
            // Other error.
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(400)
                        .header("Content-Type", "application/json")
                        .body("body");
                }),
                Some(StatusCode::BAD_REQUEST),
                false,
            ),
        ];

        for (mock_config, expected_bad_status, expected_is_desync) in cases {
            let server = MockServer::start_async().await;
            let mock = server.mock_async(mock_config).await;

            let config = client::Config {
                base: server.base_url().parse().unwrap(),
                token: Some("SOMEGARBAGETOKEN".to_string()),
                tls_options: TlsOptions::default(),
                proxy: Default::default(),
            };
            let client = Client::new(config).unwrap();
            let mut api_watcher = ApiWatcher::new(client, Pod::watch_pod_for_all_namespaces);
            let error = api_watcher
                .watch(WatchOptional {
                    allow_watch_bookmarks: Some(true),
                    field_selector: None,
                    label_selector: None,
                    resource_version: Some(""),
                    timeout_seconds: Some(300),
                    pretty: None,
                })
                .await
                .err()
                .expect("expected an invocation error here");

            let (actual_status, actual_is_desync) = match error {
                watcher::invocation::Error::Desync {
                    source: invocation::Error::BadStatus { status },
                } => (Some(status), true),
                watcher::invocation::Error::Desync { .. } => (None, true),
                watcher::invocation::Error::Recoverable {
                    source: invocation::Error::BadStatus { status },
                } => (Some(status), false),
                watcher::invocation::Error::Recoverable { .. } => (None, false),
                watcher::invocation::Error::Other {
                    source: invocation::Error::BadStatus { status },
                } => (Some(status), false),
                watcher::invocation::Error::Other { .. } => (None, false),
            };

            assert_eq!(
                actual_status, expected_bad_status,
                "actual left, expected right"
            );
            assert_eq!(
                actual_is_desync, expected_is_desync,
                "actual left, expected right"
            );
            mock.assert_async().await;
        }
    }

    /// Test that it can handle stream errors.
    #[tokio::test]
    async fn test_stream_errors() {
        let cases: Vec<(
            Box<dyn FnOnce(When, Then)>,
            Vec<Box<dyn FnOnce(Result<WatchEvent<Pod>, watcher::stream::Error<stream::Error>>)>>,
        )> = vec![
            // Tests a healthy stream
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                                "type": "ADDED",
                                "object": {
                                    "kind": "Pod",
                                    "apiVersion": "v1",
                                    "metadata": {
                                        "uid": "uid0"
                                    }
                                }
                            }{
                                "type": "ADDED",
                                "object": {
                                    "kind": "Pod",
                                    "apiVersion": "v1",
                                    "metadata": {
                                        "uid": "uid1"
                                    }
                                }
                            }"#,
                        );
                }),
                vec![
                    Box::new(|item| {
                        assert_eq!(
                            item.unwrap(),
                            WatchEvent::Added(Pod {
                                metadata: ObjectMeta {
                                    uid: Some("uid0".to_owned()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        );
                    }),
                    Box::new(|item| {
                        assert_eq!(
                            item.unwrap(),
                            WatchEvent::Added(Pod {
                                metadata: ObjectMeta {
                                    uid: Some("uid1".to_owned()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        );
                    }),
                ],
            ),
            // Desync error at start of stream.
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                                "type": "ERROR",
                                "object": {
                                    "apiVersion": "v1",
                                    "code": 410,
                                    "kind": "Status",
                                    "message": "too old resource version: 12122 (359817167)",
                                    "metadata": {},
                                    "reason": "Gone",
                                    "status": "Failure"
                                }
                            }"#,
                        );
                }),
                vec![Box::new(|item| {
                    let error = item.unwrap_err();
                    assert_matches!(
                        error,
                        watcher::stream::Error::Desync {
                            source: stream::Error::Desync
                        }
                    )
                })],
            ),
            // Desync error mid-stream.
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                                "type": "ADDED",
                                "object": {
                                    "kind": "Pod",
                                    "apiVersion": "v1",
                                    "metadata": {
                                        "uid": "uid0"
                                    }
                                }
                            }{
                                "type": "ERROR",
                                "object": {
                                    "apiVersion": "v1",
                                    "code": 410,
                                    "kind": "Status",
                                    "message": "too old resource version: 12122 (359817167)",
                                    "metadata": {},
                                    "reason": "Gone",
                                    "status": "Failure"
                                }
                            }"#,
                        );
                }),
                vec![
                    Box::new(|item| {
                        assert_eq!(
                            item.unwrap(),
                            WatchEvent::Added(Pod {
                                metadata: ObjectMeta {
                                    uid: Some("uid0".to_owned()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        );
                    }),
                    Box::new(|item| {
                        let error = item.unwrap_err();
                        assert_matches!(
                            error,
                            watcher::stream::Error::Desync {
                                source: stream::Error::Desync
                            }
                        )
                    }),
                ],
            ),
            // Desync error with items after it.
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                                "type": "ERROR",
                                "object": {
                                    "apiVersion": "v1",
                                    "code": 410,
                                    "kind": "Status",
                                    "message": "too old resource version: 12122 (359817167)",
                                    "metadata": {},
                                    "reason": "Gone",
                                    "status": "Failure"
                                }
                            }{
                                "type": "ADDED",
                                "object": {
                                    "kind": "Pod",
                                    "apiVersion": "v1",
                                    "metadata": {
                                        "uid": "uid0"
                                    }
                                }
                            }"#,
                        );
                }),
                vec![
                    Box::new(|item| {
                        let error = item.unwrap_err();
                        assert_matches!(
                            error,
                            watcher::stream::Error::Desync {
                                source: stream::Error::Desync
                            }
                        )
                    }),
                    Box::new(|item| {
                        assert_eq!(
                            item.unwrap(),
                            WatchEvent::Added(Pod {
                                metadata: ObjectMeta {
                                    uid: Some("uid0".to_owned()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                        );
                    }),
                ],
            ),
            // Non-desync Stream Error
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                            "type": "ERROR",
                            "object": {
                                "apiVersion": "v1",
                                "code": 500,
                                "kind": "Status",
                                "message": "Internal Server Error",
                                "metadata": {},
                                "reason": "Puter go BOOM",
                                "status": "Failure"
                            }
                        }"#,
                        );
                }),
                vec![Box::new(|item| {
                    assert_eq!(
                        item.unwrap(),
                        WatchEvent::ErrorStatus(Status {
                            code: Some(500),
                            message: Some("Internal Server Error".to_owned()),
                            reason: Some("Puter go BOOM".to_owned()),
                            status: Some("Failure".to_owned()),
                            ..Default::default()
                        }),
                    );
                })],
            ),
            // No body in response
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200).header("Content-Type", "application/json");
                }),
                vec![],
            ),
            // Bad JSON from API
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(r#"not valid json"#);
                }),
                vec![Box::new(|item| {
                    let error = item.unwrap_err();
                    assert_matches!(error, watcher::stream::Error::Recoverable {
                            source:
                                api_watcher::stream::Error::K8sStream {
                                    source: crate::kubernetes::stream::Error::Parsing { source },
                                },
                        } if format!("{:?}", source)
                            == r#"Json(Error("expected ident", line: 1, column: 2))"#)
                })],
            ),
            // Valid JSON of Invalid Response API
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(r#"{"a":"b"}"#);
                }),
                vec![Box::new(|item| {
                    let error = item.unwrap_err();
                    assert_matches!(error, watcher::stream::Error::Recoverable {
                            source:
                                api_watcher::stream::Error::K8sStream {
                                    source: crate::kubernetes::stream::Error::Parsing { source },
                                },
                        } if format!("{:?}", source)
                            == r#"Json(Error("missing field `type`", line: 1, column: 9))"#)
                })],
            ),
            // Non-standard object type
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                                "type": "nonstandard_type",
                                "object": {
                                    "kind": "Status",
                                    "apiVersion": "v1",
                                    "metadata": {
                                        "uid": "uid0"
                                    }
                                }
                            }"#,
                        );
                }),
                vec![Box::new(|item| {
                    let error = item.unwrap_err();
                    assert_matches!(error, watcher::stream::Error::Recoverable {
                            source:
                                api_watcher::stream::Error::K8sStream {
                                    source: crate::kubernetes::stream::Error::Parsing { source },
                                },
                        } if format!("{:?}", source)
                            == r#"Json(Error("unknown variant `nonstandard_type`, expected one of `ADDED`, `DELETED`, `MODIFIED`, `BOOKMARK`, `ERROR`", line: 2, column: 58))"#)
                })],
            ),
            // Incorrect object type
            (
                Box::new(|when, then| {
                    when.method(GET).path("/api/v1/pods");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .body(
                            r#"{
                                "type": "MODIFIED",
                                "object": {
                                    "kind": "StatefulSet",
                                    "apiVersion": "v1",
                                    "metadata": {
                                        "uid": "uid0"
                                    }
                                }
                            }"#,
                        );
                }),
                vec![Box::new(|item| {
                    let error = item.unwrap_err();
                    assert_matches!(error, watcher::stream::Error::Recoverable {
                            source:
                                api_watcher::stream::Error::K8sStream {
                                    source: crate::kubernetes::stream::Error::Parsing { source },
                                },
                        } if format!("{:?}", source)
                            == r#"Json(Error("invalid value: string \"StatefulSet\", expected Pod", line: 10, column: 29))"#)
                })],
            ),
        ];

        for (mock_config, assertions) in cases {
            let server = MockServer::start_async().await;
            let mock = server.mock_async(mock_config).await;

            let config = client::Config {
                base: server.base_url().parse().unwrap(),
                token: Some("SOMEGARBAGETOKEN".to_string()),
                tls_options: TlsOptions::default(),
                proxy: Default::default(),
            };
            let client = Client::new(config).unwrap();
            let mut api_watcher = ApiWatcher::new(client, Pod::watch_pod_for_all_namespaces);
            let mut stream = api_watcher
                .watch(WatchOptional {
                    allow_watch_bookmarks: Some(true),
                    field_selector: None,
                    label_selector: None,
                    resource_version: Some(""),
                    timeout_seconds: Some(300),
                    pretty: None,
                })
                .await
                .expect("no invocation error is supposed to happen in this test");

            for assertion in assertions {
                let item = stream
                    .next()
                    .await
                    .expect("we have an assertion, but an item wasn't available");
                assertion(item);
            }
            assert!(stream.next().await.is_none(), "expected to cover the whole stream with assertion, but got some items after all assertions passed");
            mock.assert_async().await;
        }
    }
}
