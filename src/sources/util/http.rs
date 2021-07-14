use crate::{
    config::SourceContext,
    internal_events::{HttpBadRequest, HttpDecompressError, HttpEventsReceived},
    tls::{MaybeTlsSettings, TlsConfig},
    Pipeline,
};
use async_trait::async_trait;
use bytes::{Buf, Bytes};
use flate2::read::{DeflateDecoder, MultiGzDecoder};
use futures::{FutureExt, SinkExt, StreamExt, TryFutureExt};
use headers::{Authorization, HeaderMapExt};
use serde::{Deserialize, Serialize};
use snap::raw::Decoder as SnappyDecoder;
use std::{
    collections::HashMap, convert::TryFrom, error::Error, fmt, io::Read, net::SocketAddr, sync::Arc,
};
use vector_core::event::{BatchNotifier, BatchStatus, BatchStatusReceiver, Event};
use warp::{
    filters::{path::FullPath, path::Tail, BoxedFilter},
    http::{HeaderMap, StatusCode},
    reject::Rejection,
    Filter,
};

#[cfg(any(feature = "sources-http", feature = "sources-heroku_logs"))]
pub(crate) fn add_query_parameters(
    mut events: Vec<Event>,
    query_parameters_config: &[String],
    query_parameters: HashMap<String, String>,
) -> Vec<Event> {
    for query_parameter_name in query_parameters_config {
        let value = query_parameters.get(query_parameter_name);
        for event in events.iter_mut() {
            event.as_mut_log().insert(
                query_parameter_name as &str,
                crate::event::Value::from(value.map(String::to_owned)),
            );
        }
    }

    events
}

#[derive(Serialize, Debug)]
pub struct ErrorMessage {
    code: u16,
    message: String,
}
impl ErrorMessage {
    pub fn new(code: StatusCode, message: String) -> Self {
        ErrorMessage {
            code: code.as_u16(),
            message,
        }
    }
}
impl Error for ErrorMessage {}
impl fmt::Display for ErrorMessage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl warp::reject::Reject for ErrorMessage {}

struct RejectShuttingDown;
impl fmt::Debug for RejectShuttingDown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("shutting down")
    }
}
impl warp::reject::Reject for RejectShuttingDown {}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct HttpSourceAuthConfig {
    pub username: String,
    pub password: String,
}

impl TryFrom<Option<&HttpSourceAuthConfig>> for HttpSourceAuth {
    type Error = String;

    fn try_from(auth: Option<&HttpSourceAuthConfig>) -> Result<Self, Self::Error> {
        match auth {
            Some(auth) => {
                let mut headers = HeaderMap::new();
                headers.typed_insert(Authorization::basic(&auth.username, &auth.password));
                match headers.get("authorization") {
                    Some(value) => {
                        let token = value
                            .to_str()
                            .map_err(|error| format!("Failed stringify HeaderValue: {:?}", error))?
                            .to_owned();
                        Ok(HttpSourceAuth { token: Some(token) })
                    }
                    None => Err("Authorization headers wasn't generated".to_owned()),
                }
            }
            None => Ok(HttpSourceAuth { token: None }),
        }
    }
}

#[derive(Debug, Clone)]
struct HttpSourceAuth {
    pub token: Option<String>,
}

impl HttpSourceAuth {
    pub fn is_valid(&self, header: &Option<String>) -> Result<(), ErrorMessage> {
        match (&self.token, header) {
            (Some(token1), Some(token2)) => {
                if token1 == token2 {
                    Ok(())
                } else {
                    Err(ErrorMessage::new(
                        StatusCode::UNAUTHORIZED,
                        "Invalid username/password".to_owned(),
                    ))
                }
            }
            (Some(_), None) => Err(ErrorMessage::new(
                StatusCode::UNAUTHORIZED,
                "No authorization header".to_owned(),
            )),
            (None, _) => Ok(()),
        }
    }
}

pub fn decode(header: &Option<String>, mut body: Bytes) -> Result<Bytes, ErrorMessage> {
    if let Some(encodings) = header {
        for encoding in encodings.rsplit(',').map(str::trim) {
            body = match encoding {
                "identity" => body,
                "gzip" => {
                    let mut decoded = Vec::new();
                    MultiGzDecoder::new(body.reader())
                        .read_to_end(&mut decoded)
                        .map_err(|error| handle_decode_error(encoding, error))?;
                    decoded.into()
                }
                "deflate" => {
                    let mut decoded = Vec::new();
                    DeflateDecoder::new(body.reader())
                        .read_to_end(&mut decoded)
                        .map_err(|error| handle_decode_error(encoding, error))?;
                    decoded.into()
                }
                "snappy" => SnappyDecoder::new()
                    .decompress_vec(&body)
                    .map_err(|error| handle_decode_error(encoding, error))?
                    .into(),
                encoding => {
                    return Err(ErrorMessage::new(
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        format!("Unsupported encoding {}", encoding),
                    ))
                }
            }
        }
    }

    Ok(body)
}

fn handle_decode_error(encoding: &str, error: impl std::error::Error) -> ErrorMessage {
    emit!(HttpDecompressError {
        encoding,
        error: &error
    });
    ErrorMessage::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!("Failed decompressing payload with {} decoder.", encoding),
    )
}

#[async_trait]
pub trait HttpSource: Clone + Send + Sync + 'static {
    fn build_events(
        &self,
        body: Bytes,
        header_map: HeaderMap,
        query_parameters: HashMap<String, String>,
        path: &str,
    ) -> Result<Vec<Event>, ErrorMessage>;

    fn run(
        self,
        address: SocketAddr,
        path: &str,
        strict_path: bool,
        tls: &Option<TlsConfig>,
        auth: &Option<HttpSourceAuthConfig>,
        cx: SourceContext,
    ) -> crate::Result<crate::sources::Source> {
        let tls = MaybeTlsSettings::from_config(tls, true)?;
        let auth = HttpSourceAuth::try_from(auth.as_ref())?;
        let path = path.to_owned();
        let out = cx.out;
        let shutdown = cx.shutdown;
        let acknowledgements = cx.acknowledgements;
        Ok(Box::pin(async move {
            let span = crate::trace::current_span();
            let mut filter: BoxedFilter<()> = warp::post().boxed();
            for s in path.split('/').filter(|&x| !x.is_empty()) {
                filter = filter.and(warp::path(s.to_string())).boxed()
            }
            let svc = filter
                .and(warp::path::tail())
                .and_then(move |tail: Tail| async move {
                    if !strict_path || tail.as_str().is_empty() {
                        Ok(())
                    } else {
                        debug!(message = "Path rejected.");
                        Err(warp::reject::custom(ErrorMessage::new(
                            StatusCode::NOT_FOUND,
                            "Not found".to_string(),
                        )))
                    }
                })
                .untuple_one()
                .and(warp::path::full())
                .and(warp::header::optional::<String>("authorization"))
                .and(warp::header::optional::<String>("content-encoding"))
                .and(warp::header::headers_cloned())
                .and(warp::body::bytes())
                .and(warp::query::<HashMap<String, String>>())
                .and_then(
                    move |path: FullPath,
                          auth_header,
                          encoding_header,
                          headers: HeaderMap,
                          body: Bytes,
                          query_parameters: HashMap<String, String>| {
                        debug!(message = "Handling HTTP request.", headers = ?headers);

                        let events = auth
                            .is_valid(&auth_header)
                            .and_then(|()| decode(&encoding_header, body))
                            .and_then(|body| {
                                let body_len = body.len();
                                self.build_events(body, headers, query_parameters, path.as_str())
                                    .map(|events| (events, body_len))
                            });

                        handle_request(events, acknowledgements, out.clone())
                    },
                )
                .with(warp::trace(move |_info| span.clone()));

            let ping = warp::get().and(warp::path("ping")).map(|| "pong");
            let routes = svc.or(ping).recover(|r: Rejection| async move {
                if let Some(e_msg) = r.find::<ErrorMessage>() {
                    let json = warp::reply::json(e_msg);
                    Ok(warp::reply::with_status(
                        json,
                        StatusCode::from_u16(e_msg.code)
                            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    ))
                } else {
                    //other internal error - will return 500 internal server error
                    Err(r)
                }
            });

            info!(message = "Building HTTP server.", address = %address);

            let listener = tls.bind(&address).await.unwrap();
            warp::serve(routes)
                .serve_incoming_with_graceful_shutdown(
                    listener.accept_stream(),
                    shutdown.map(|_| ()),
                )
                .await;
            Ok(())
        }))
    }
}

async fn handle_request(
    events: Result<(Vec<Event>, usize), ErrorMessage>,
    acknowledgements: bool,
    mut out: Pipeline,
) -> Result<impl warp::Reply, Rejection> {
    match events {
        Ok((mut events, body_size)) => {
            emit!(HttpEventsReceived {
                events_count: events.len(),
                byte_size: body_size,
            });

            let receiver = acknowledgements.then(|| {
                let (batch, receiver) = BatchNotifier::new_with_receiver();
                for event in &mut events {
                    event.add_batch_notifier(Arc::clone(&batch));
                }
                receiver
            });

            out.send_all(&mut futures::stream::iter(events).map(Ok))
                .map_err(move |error: crate::pipeline::ClosedError| {
                    // can only fail if receiving end disconnected, so we are shutting down,
                    // probably not gracefully.
                    error!(message = "Failed to forward events, downstream is closed.");
                    error!(message = "Tried to send the following event.", %error);
                    warp::reject::custom(RejectShuttingDown)
                })
                .and_then(|_| handle_batch_status(receiver))
                .await
        }
        Err(error) => {
            emit!(HttpBadRequest {
                error_code: error.code,
                error_message: error.message.as_str(),
            });
            Err(warp::reject::custom(error))
        }
    }
}

async fn handle_batch_status(
    receiver: Option<BatchStatusReceiver>,
) -> Result<impl warp::Reply, Rejection> {
    match receiver {
        None => Ok(warp::reply()),
        Some(receiver) => match receiver.await {
            BatchStatus::Delivered => Ok(warp::reply()),
            BatchStatus::Errored => Err(warp::reject::custom(ErrorMessage::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error delivering contents to sink".into(),
            ))),
            BatchStatus::Failed => Err(warp::reject::custom(ErrorMessage::new(
                StatusCode::BAD_REQUEST,
                "Contents failed to deliver to sink".into(),
            ))),
        },
    }
}
