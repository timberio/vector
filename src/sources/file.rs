use crate::event::{self, Event};
use bytes::Bytes;
use file_source::file_server::FileServer;
use futures::{future, sync::mpsc, Future, Sink};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime};
use string_cache::DefaultAtom as Atom;
use tokio_trace::{dispatcher, field};

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields, default)]
pub struct FileConfig {
    pub include: Vec<PathBuf>,
    pub exclude: Vec<PathBuf>,
    pub context_key: Option<String>,
    pub start_at_beginning: bool,
    pub ignore_older: Option<u64>,
    #[serde(default = "default_max_line_bytes")]
    pub max_line_bytes: usize,
    pub host_key: Option<String>,
}

fn default_max_line_bytes() -> usize {
    100 * 1024
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            include: vec![],
            exclude: vec![],
            context_key: Some("file".to_string()),
            start_at_beginning: false,
            ignore_older: None,
            max_line_bytes: default_max_line_bytes(),
            host_key: None,
        }
    }
}

#[typetag::serde(name = "file")]
impl crate::topology::config::SourceConfig for FileConfig {
    fn build(&self, out: mpsc::Sender<Event>) -> Result<super::Source, String> {
        // TODO: validate paths
        Ok(file_source(self, out))
    }
}

pub fn file_source(config: &FileConfig, out: mpsc::Sender<Event>) -> super::Source {
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();

    let ignore_before = config
        .ignore_older
        .map(|secs| SystemTime::now() - Duration::from_secs(secs));

    let file_server = FileServer {
        include: config.include.clone(),
        exclude: config.exclude.clone(),
        max_read_bytes: 2048,
        start_at_beginning: config.start_at_beginning,
        ignore_before,
        max_line_bytes: config.max_line_bytes,
    };

    let context_key = config.context_key.clone().map(Atom::from);
    let host_key = config.host_key.clone().unwrap_or(event::HOST.to_string());
    let hostname = hostname::get_hostname();

    let out = out
        .sink_map_err(|_| ())
        .with(move |(line, file): (Bytes, String)| {
            trace!(message = "Recieved one event.", file = file.as_str());
            let mut event = Event::from(line);

            if let Some(context_key) = &context_key {
                event
                    .as_mut_log()
                    .insert_implicit(context_key.clone(), file.into());
            }

            if let Some(hostname) = &hostname {
                event
                    .as_mut_log()
                    .insert_implicit(host_key.clone().into(), hostname.clone().into());
            }

            future::ok(event)
        });

    let include = config.include.clone();
    let exclude = config.exclude.clone();
    Box::new(future::lazy(move || {
        info!(
            message = "Starting file server.",
            include = field::debug(include),
            exclude = field::debug(exclude)
        );

        let span = info_span!("file-server");
        let dispatcher = dispatcher::get_default(|d| d.clone());
        thread::spawn(move || {
            let dispatcher = dispatcher;
            dispatcher::with_default(&dispatcher, || {
                span.enter(|| {
                    file_server.run(out, shutdown_rx);
                })
            });
        });

        // Dropping shutdown_tx is how we signal to the file server that it's time to shut down,
        // so it needs to be held onto until the future we return is dropped.
        future::empty().inspect(|_| drop(shutdown_tx))
    }))
}
