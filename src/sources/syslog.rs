use crate::sources::{
    socket::{Mode, SocketConfig},
    util::StreamDecoder,
};
use crate::Value;
use crate::{
    config::{
        log_schema, DataType, GenerateConfig, Resource, SourceConfig, SourceContext,
        SourceDescription,
    },
    transforms::{
        add_fields::AddFields,
        remap::{Remap, RemapConfig},
    },
};
use indoc::indoc;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug)]
pub struct SyslogConfig {
    #[serde(flatten)]
    original_config: SocketConfig,
}

impl From<SocketConfig> for SyslogConfig {
    fn from(config: SocketConfig) -> Self {
        SyslogConfig {
            original_config: config,
        }
    }
}

inventory::submit! {
    SourceDescription::new::<SyslogConfig>("syslog")
}

impl GenerateConfig for SyslogConfig {
    fn generate_config() -> toml::Value {
        SocketConfig::generate_config()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "syslog")]
impl SourceConfig for SyslogConfig {
    async fn build(&self, mut cx: SourceContext) -> crate::Result<super::Source> {
        let conf = RemapConfig {
            source: indoc! {r#"
                structured = parse_syslog!(.message)
                . = merge(., structured)
            "#}
            .to_string(),
            drop_on_abort: false,
            drop_on_error: false,
        };

        let tf = Remap::new(conf).unwrap();
        cx.out.register_transform(Box::new(tf));

        let src_field = AddFields::new(
            indexmap::indexmap! {
                log_schema().source_type_key().to_string() => Value::from("syslog".to_string()),
            },
            true,
        )
        .unwrap();
        cx.out.register_transform(Box::new(src_field));

        // Enforce the syslog required decoding per socket type
        match self.original_config.mode.clone() {
            Mode::Tcp(mut config) => {
                config.set_decoder(Some(StreamDecoder::SyslogDecoder(
                    codec::SyslogDecoder::new(config.max_length()),
                )));
                SocketConfig::new_tcp(config).build(cx).await
            }
            //Mode::Udp(config) => {
            //    run a udp source producing an event per datagram
            //},
            //Mode::UnixStream(config) => {
            //    run a unix stream source with the correct decoder
            //},
            //Mode::UnixDatagram(config) => {
            //    note: the original syslog source did not support that
            //    run an af_unix source producing an event per datagram
            //},
            _ => self.original_config.build(cx).await,
        }
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "syslog"
    }

    fn resources(&self) -> Vec<Resource> {
        self.original_config.resources()
    }
}

#[cfg(test)]
mod test {
    use super::SyslogConfig;
    use crate::sources::socket::Mode;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<SyslogConfig>();
    }

    #[test]
    fn config_tcp() {
        let config: SyslogConfig = toml::from_str(
            r#"
            mode = "tcp"
            address = "127.0.0.1:1235"
          "#,
        )
        .unwrap();
        assert!(matches!(config.original_config.mode, Mode::Tcp(_)));
    }
}
