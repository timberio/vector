#![recursion_limit = "256"] // for async-stream
#![allow(clippy::approx_constant)]
#![allow(clippy::float_cmp)]
#![allow(clippy::blocks_in_if_conditions)]
#![allow(clippy::match_wild_err_arm)]
#![allow(clippy::new_ret_no_self)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::trivial_regex)]
#![allow(clippy::type_complexity)]
#![allow(clippy::unit_arg)]
#![deny(clippy::clone_on_ref_ptr)]
#![deny(clippy::trivially_copy_pass_by_ref)]

#[macro_use]
extern crate tracing;
#[macro_use]
extern crate derivative;
#[cfg(feature = "vrl-cli")]
extern crate vrl_cli;

#[macro_use]
pub mod config;
pub mod cli;
pub mod codecs;
pub mod conditions;
pub mod dns;
#[cfg(feature = "docker")]
pub mod docker;
pub mod expiring_hash_map;
pub mod framers;
pub mod generate;
#[cfg(feature = "wasm")]
pub mod wasm;
#[macro_use]
pub mod internal_events;
#[cfg(feature = "api")]
pub mod api;
pub mod app;
pub mod async_read;
pub mod buffers;
pub mod encoding_transcode;
pub mod heartbeat;
pub mod http;
#[cfg(any(feature = "sources-kafka", feature = "sinks-kafka"))]
pub mod kafka;
pub mod kubernetes;
pub mod line_agg;
pub mod list;
pub(crate) mod pipeline;
pub(crate) mod proto;
pub mod providers;
#[cfg(feature = "rusoto_core")]
pub mod rusoto;
pub mod serde;
pub mod service;
pub mod shutdown;
pub mod signal;
pub mod sink;
pub mod sinks;
pub mod sources;
pub(crate) mod stats;
pub mod stream;
#[cfg(feature = "api-client")]
mod tap;
pub mod tcp;
pub mod template;
pub mod test_util;
pub mod tls;
#[cfg(feature = "api-client")]
pub mod top;
pub mod topology;
pub mod trace;
pub mod transforms;
pub mod trigger;
pub mod types;
pub mod udp;
pub mod unit_test;
pub(crate) mod utilization;
pub mod validate;
#[cfg(windows)]
pub mod vector_windows;

pub use pipeline::Pipeline;

pub use vector_core::{event, mapping, metrics, transform, Error, Result};

pub fn vector_version() -> impl std::fmt::Display {
    #[cfg(feature = "nightly")]
    let pkg_version = format!("{}-nightly", built_info::PKG_VERSION);

    #[cfg(not(feature = "nightly"))]
    let pkg_version = built_info::PKG_VERSION;

    pkg_version
}

pub fn get_version() -> String {
    let pkg_version = vector_version();
    let build_desc = built_info::VECTOR_BUILD_DESC;
    let build_string = match build_desc {
        Some(desc) => format!("{} {}", built_info::TARGET, desc),
        None => built_info::TARGET.into(),
    };

    // We do not add 'debug' to the BUILD_DESC unless the caller has flagged on line
    // or full debug symbols. See the Cargo Book profiling section for value meaning:
    // https://doc.rust-lang.org/cargo/reference/profiles.html#debug
    let build_string = match built_info::DEBUG {
        "1" => format!("{} debug=line", build_string),
        "2" | "true" => format!("{} debug=full", build_string),
        _ => build_string,
    };

    format!("{} ({})", pkg_version, build_string)
}

#[allow(unused)]
pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

pub fn get_hostname() -> std::io::Result<String> {
    Ok(hostname::get()?.to_string_lossy().into())
}
