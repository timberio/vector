use crate::{
    cli::{handle_config_errors, Color, LogFormat, Opts, RootOpts, SubCommand},
    config, generate, heartbeat, list, metrics,
    signal::{self, SignalTo},
    topology::{self, RunningTopology},
    trace, unit_test, validate,
};
use cfg_if::cfg_if;
use futures::StreamExt;
use std::{collections::HashMap, path::PathBuf};
use tokio::{
    runtime::{self, Runtime},
    sync::mpsc,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(feature = "api-client")]
use crate::tap;
#[cfg(feature = "api-client")]
use crate::top;
#[cfg(feature = "api")]
use crate::{api, internal_events::ApiStarted};

#[cfg(windows)]
use crate::service;

use crate::internal_events::{
    VectorConfigLoadFailed, VectorQuit, VectorRecoveryFailed, VectorReloadFailed, VectorReloaded,
    VectorStarted, VectorStopped,
};

pub struct ApplicationConfig {
    pub config_paths: Vec<config::ConfigPath>,
    pub topology: RunningTopology,
    pub graceful_crash: mpsc::UnboundedReceiver<()>,
    #[cfg(feature = "api")]
    pub api: config::api::Options,
    pub signal_handler: signal::SignalHandler,
    pub signal_rx: signal::SignalRx,
}

pub struct Application {
    opts: RootOpts,
    pub config: ApplicationConfig,
    pub runtime: Runtime,
}

impl Application {
    pub fn prepare() -> Result<Self, exitcode::ExitCode> {
        let opts = Opts::get_matches();
        Self::prepare_from_opts(opts)
    }

    pub fn prepare_from_opts(opts: Opts) -> Result<Self, exitcode::ExitCode> {
        openssl_probe::init_ssl_cert_env_vars();

        let level = std::env::var("LOG").unwrap_or_else(|_| match opts.log_level() {
            "off" => "off".to_owned(),
            level => [
                format!("vector={}", level),
                format!("codec={}", level),
                format!("vrl={}", level),
                format!("file_source={}", level),
                "tower_limit=trace".to_owned(),
                format!("rdkafka={}", level),
                format!("lapin={}", level),
            ]
            .join(","),
        });

        let root_opts = opts.root;

        let sub_command = opts.sub_command;

        let color = match root_opts.color {
            #[cfg(unix)]
            Color::Auto => atty::is(atty::Stream::Stdout),
            #[cfg(windows)]
            Color::Auto => false, // ANSI colors are not supported by cmd.exe
            Color::Always => true,
            Color::Never => false,
        };

        let json = match &root_opts.log_format {
            LogFormat::Text => false,
            LogFormat::Json => true,
        };

        let enable_datadog_tracing = root_opts.enable_datadog_tracing;

        metrics::init().expect("metrics initialization failed");

        if let Some(threads) = root_opts.threads {
            if threads < 1 {
                // Unforunately we can't `error!` here because we haven't yet called `trace::init`,
                // and we can't call that without a runtime available.
                println!("Config error: The `threads` argument must be greater or equal to 1.");
                return Err(exitcode::CONFIG);
            }
        }

        let rt = {
            runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Unable to create async runtime")
        };

        // We need a runtime to initiate the datadog tracing exporter
        rt.block_on(async move {
            trace::init(color, json, &level, enable_datadog_tracing);
            info!(
                message = "Log level is enabled.",
                ?level,
                ?enable_datadog_tracing
            );
        });

        let config = {
            let config_paths = root_opts.config_paths_with_formats();
            let watch_config = root_opts.watch_config;
            let require_healthy = root_opts.require_healthy;

            rt.block_on(async move {
                // Signal handler for OS and provider messages.
                let (mut signal_handler, signal_rx) = signal::SignalHandler::new();
                signal_handler.forever(signal::os_signals());

                if let Some(s) = sub_command {
                    let code = match s {
                        SubCommand::Validate(v) => validate::validate(&v, color).await,
                        SubCommand::List(l) => list::cmd(&l),
                        SubCommand::Test(t) => unit_test::cmd(&t).await,
                        SubCommand::Generate(g) => generate::cmd(&g),
                        #[cfg(feature = "api-client")]
                        SubCommand::Top(t) => top::cmd(&t).await,
                        #[cfg(feature = "api-client")]
                        SubCommand::Tap(t) => tap::cmd(&t).await,
                        #[cfg(windows)]
                        SubCommand::Service(s) => service::cmd(&s),
                        #[cfg(feature = "vrl-cli")]
                        SubCommand::Vrl(s) => vrl_cli::cmd::cmd(&s),
                    };

                    return Err(code);
                };

                let config_paths = config::process_paths(&config_paths).ok_or(exitcode::CONFIG)?;

                if watch_config {
                    // Start listening for config changes immediately.
                    config::watcher::spawn_thread(config_paths.iter().map(Into::into), None)
                        .map_err(|error| {
                            error!(message = "Unable to start config watcher.", %error);
                            exitcode::CONFIG
                        })?;
                }

                info!(
                    message = "Loading configs.",
                    paths = ?config_paths.iter().map(<&PathBuf>::from).collect::<Vec<_>>()
                );

                config::init_log_schema(&config_paths, true).map_err(handle_config_errors)?;

                let mut config =
                    config::load_from_paths_with_provider(&config_paths, &mut signal_handler)
                        .await
                        .map_err(handle_config_errors)?;

                if !config.healthchecks.enabled {
                    info!("Health checks are disabled.");
                }
                config.healthchecks.set_require_healthy(require_healthy);

                let diff = config::ConfigDiff::initial(&config);
                let pieces = topology::build_or_log_errors(&config, &diff, HashMap::new())
                    .await
                    .ok_or(exitcode::CONFIG)?;

                #[cfg(feature = "api")]
                let api = config.api;

                let result = topology::start_validated(config, diff, pieces).await;
                let (topology, graceful_crash) = result.ok_or(exitcode::CONFIG)?;

                Ok(ApplicationConfig {
                    config_paths,
                    topology,
                    graceful_crash,
                    #[cfg(feature = "api")]
                    api,
                    signal_handler,
                    signal_rx,
                })
            })
        }?;

        Ok(Application {
            opts: root_opts,
            config,
            runtime: rt,
        })
    }

    pub fn run(self) {
        let rt = self.runtime;

        let mut graceful_crash = UnboundedReceiverStream::new(self.config.graceful_crash);
        let mut topology = self.config.topology;

        let mut config_paths = self.config.config_paths;

        let opts = self.opts;

        #[cfg(feature = "api")]
        let api_config = self.config.api;

        let mut signal_handler = self.config.signal_handler;
        let mut signal_rx = self.config.signal_rx;

        // Any internal_logs sources will have grabbed a copy of the
        // early buffer by this point and set up a subscriber.
        crate::trace::stop_buffering();

        rt.block_on(async move {
            emit!(VectorStarted);
            tokio::spawn(heartbeat::heartbeat());

            // Configure the API server, if applicable.
            cfg_if! (
                if #[cfg(feature = "api")] {
                    // Assigned to prevent the API terminating when falling out of scope.
                    let api_server = if api_config.enabled {
                        emit!(ApiStarted {
                            addr: api_config.address.unwrap(),
                            playground: api_config.playground
                        });

                        Some(api::Server::start(topology.config(), topology.watch()))
                    } else {
                        info!(message="API is disabled, enable by setting `api.enabled` to `true` and use commands like `vector top`.");
                        None
                    };
                }
            );

            let mut sources_finished = topology.sources_finished();

            let signal = loop {
                tokio::select! {
                    Some(signal) = signal_rx.recv() => {
                        match signal {
                            SignalTo::ReloadFromConfigBuilder(config_builder) => {
                                match config_builder.build().map_err(handle_config_errors) {
                                    Ok(mut new_config) => {
                                        new_config.healthchecks.set_require_healthy(opts.require_healthy);
                                        match topology
                                            .reload_config_and_respawn(new_config)
                                            .await
                                        {
                                            Ok(true) => {
                                                #[cfg(feature = "api")]
                                                // Pass the new config to the API server.
                                                if let Some(ref api_server) = api_server {
                                                    api_server.update_config(topology.config());
                                                }

                                                emit!(VectorReloaded { config_paths: &config_paths })
                                            },
                                            Ok(false) => emit!(VectorReloadFailed),
                                            // Trigger graceful shutdown for what remains of the topology
                                            Err(()) => {
                                                emit!(VectorReloadFailed);
                                                emit!(VectorRecoveryFailed);
                                                break SignalTo::Shutdown;
                                            }
                                        }
                                        sources_finished = topology.sources_finished();
                                    },
                                    Err(_) => {
                                        emit!(VectorConfigLoadFailed);
                                    }
                                }
                            }
                            SignalTo::ReloadFromDisk => {
                                // Reload paths
                                config_paths = config::process_paths(&opts.config_paths_with_formats()).unwrap_or(config_paths);
                                // Reload config
                                let new_config = config::load_from_paths_with_provider(&config_paths, &mut signal_handler)
                                    .await
                                    .map_err(handle_config_errors).ok();

                                if let Some(mut new_config) = new_config {
                                    new_config.healthchecks.set_require_healthy(opts.require_healthy);
                                    match topology
                                        .reload_config_and_respawn(new_config)
                                        .await
                                    {
                                        Ok(true) => {
                                            #[cfg(feature = "api")]
                                            // Pass the new config to the API server.
                                            if let Some(ref api_server) = api_server {
                                                api_server.update_config(topology.config());
                                            }

                                            emit!(VectorReloaded { config_paths: &config_paths })
                                        },
                                        Ok(false) => emit!(VectorReloadFailed),
                                        // Trigger graceful shutdown for what remains of the topology
                                        Err(()) => {
                                            emit!(VectorReloadFailed);
                                            emit!(VectorRecoveryFailed);
                                            break SignalTo::Shutdown;
                                        }
                                    }
                                    sources_finished = topology.sources_finished();
                                } else {
                                    emit!(VectorConfigLoadFailed);
                                }
                            }
                            _ => break signal,
                        }
                    }
                    // Trigger graceful shutdown if a component crashed, or all sources have ended.
                    _ = graceful_crash.next() => break SignalTo::Shutdown,
                    _ = &mut sources_finished => break SignalTo::Shutdown,
                    else => unreachable!("Signal streams never end"),
                }
            };

            match signal {
                SignalTo::Shutdown => {
                    emit!(VectorStopped);
                    tokio::select! {
                        _ = topology.stop() => (), // Graceful shutdown finished
                        _ = signal_rx.recv() => {
                            // It is highly unlikely that this event will exit from topology.
                            emit!(VectorQuit);
                            // Dropping the shutdown future will immediately shut the server down
                        }
                    }
                }
                SignalTo::Quit => {
                    // It is highly unlikely that this event will exit from topology.
                    emit!(VectorQuit);
                    drop(topology);
                }
                _ => unreachable!(),
            }

            opentelemetry::global::shutdown_tracer_provider();
        });
    }
}
