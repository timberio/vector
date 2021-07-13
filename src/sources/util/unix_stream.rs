use crate::{
    async_read::VecAsyncReadExt,
    emit,
    event::Event,
    internal_events::{ConnectionOpen, OpenGauge, UnixSocketError, UnixSocketFileDeleteFailed},
    shutdown::ShutdownSignal,
    sources::Source,
    Pipeline,
};
use bytes::Bytes;
use futures::{FutureExt, SinkExt, StreamExt};
use std::{fs::remove_file, future::ready, path::PathBuf, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
    time::sleep,
};
use tokio_stream::wrappers::UnixListenerStream;
use tokio_util::codec::{Decoder, FramedRead};
use tracing::field;
use tracing_futures::Instrument;

/// Returns a Source object corresponding to a Unix domain stream
/// socket. Passing in different functions for build_event can allow
/// for different source-specific logic (such as decoding syslog
/// messages in the syslog source).
pub fn build_unix_stream_source<D>(
    listen_path: PathBuf,
    decoder: D,
    host_key: String,
    shutdown: ShutdownSignal,
    out: Pipeline,
    build_event: impl Fn(Bytes, &str, Option<Bytes>) -> Option<Event> + Clone + Send + Sync + 'static,
) -> Source
where
    D: Decoder + Clone + Send + 'static,
    D::Item: Into<Bytes>,
    D::Error: From<std::io::Error> + std::fmt::Debug + std::fmt::Display,
{
    let out = out.sink_map_err(|error| error!(message = "Error sending line.", %error));

    Box::pin(async move {
        let listener = UnixListener::bind(&listen_path).expect("Failed to bind to listener socket");
        info!(message = "Listening.", path = ?listen_path, r#type = "unix");

        let connection_open = OpenGauge::new();
        let stream = UnixListenerStream::new(listener).take_until(shutdown.clone());
        tokio::pin!(stream);
        while let Some(socket) = stream.next().await {
            let socket = match socket {
                Err(error) => {
                    error!(message = "Failed to accept socket.", %error);
                    continue;
                }
                Ok(socket) => socket,
            };

            let listen_path = listen_path.clone();
            let host_key = host_key.clone();

            let span = info_span!("connection");
            let path = if let Ok(addr) = socket.peer_addr() {
                if let Some(path) = addr.as_pathname().map(|e| e.to_owned()) {
                    span.record("peer_path", &field::debug(&path));
                    Some(path)
                } else {
                    None
                }
            } else {
                None
            };

            let build_event = build_event.clone();
            let received_from: Option<Bytes> =
                path.map(|p| p.to_string_lossy().into_owned().into());

            let stream = socket.allow_read_until(shutdown.clone().map(|_| ()));
            let mut stream = FramedRead::new(stream, decoder.clone()).filter_map(move |bytes| {
                ready(match bytes {
                    Ok(bytes) => {
                        build_event(bytes.into(), &host_key, received_from.clone()).map(Ok)
                    }
                    Err(error) => {
                        emit!(UnixSocketError {
                            error,
                            path: &listen_path
                        });
                        None
                    }
                })
            });

            let connection_open = connection_open.clone();
            let mut out = out.clone();
            tokio::spawn(
                async move {
                    let _open_token = connection_open.open(|count| emit!(ConnectionOpen { count }));
                    let _ = out.send_all(&mut stream).await;
                    info!("Finished sending.");

                    let socket: &mut UnixStream = stream.get_mut().get_mut().get_mut();
                    if let Err(error) = socket.shutdown().await {
                        error!(message = "Failed shutting down socket.", %error);
                    }
                }
                .instrument(span),
            );
        }

        // Cleanup
        drop(stream);

        // Wait for open connections to finish
        while connection_open.any_open() {
            sleep(Duration::from_millis(10)).await;
        }

        // Delete socket file
        if let Err(error) = remove_file(&listen_path) {
            emit!(UnixSocketFileDeleteFailed {
                path: &listen_path,
                error
            });
        }

        Ok(())
    })
}
