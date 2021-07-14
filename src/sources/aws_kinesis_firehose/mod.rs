use crate::{
    config::{DataType, GenerateConfig, Resource, SourceConfig, SourceContext, SourceDescription},
    tls::{MaybeTlsSettings, TlsConfig},
};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr};
use warp::Filter;

pub mod errors;
mod filters;
mod handlers;
mod models;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AwsKinesisFirehoseConfig {
    address: SocketAddr,
    access_key: Option<String>,
    tls: Option<TlsConfig>,
    record_compression: Option<Compression>,
}

#[derive(Derivative, Copy, Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derivative(Default)]
pub enum Compression {
    #[derivative(Default)]
    Auto,
    None,
    Gzip,
}

impl fmt::Display for Compression {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Compression::Auto => write!(fmt, "auto"),
            Compression::None => write!(fmt, "none"),
            Compression::Gzip => write!(fmt, "gzip"),
        }
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "aws_kinesis_firehose")]
impl SourceConfig for AwsKinesisFirehoseConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        let svc = filters::firehose(
            self.access_key.clone(),
            self.record_compression.unwrap_or_default(),
            cx.out,
        );

        let tls = MaybeTlsSettings::from_config(&self.tls, true)?;
        let listener = tls.bind(&self.address).await?;

        let shutdown = cx.shutdown;
        Ok(Box::pin(async move {
            let span = crate::trace::current_span();
            warp::serve(svc.with(warp::trace(move |_info| span.clone())))
                .serve_incoming_with_graceful_shutdown(
                    listener.accept_stream(),
                    shutdown.map(|_| ()),
                )
                .await;
            Ok(())
        }))
    }

    fn output_type(&self) -> DataType {
        DataType::Log
    }

    fn source_type(&self) -> &'static str {
        "aws_kinesis_firehose"
    }

    fn resources(&self) -> Vec<Resource> {
        vec![Resource::tcp(self.address)]
    }
}

inventory::submit! {
    SourceDescription::new::<AwsKinesisFirehoseConfig>("aws_kinesis_firehose")
}

impl GenerateConfig for AwsKinesisFirehoseConfig {
    fn generate_config() -> toml::Value {
        toml::Value::try_from(Self {
            address: "0.0.0.0:443".parse().unwrap(),
            access_key: None,
            tls: None,
            record_compression: None,
        })
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::Event,
        log_event,
        test_util::{collect_ready, next_addr, wait_for_tcp},
        Pipeline,
    };
    use bytes::Bytes;
    use chrono::{DateTime, SubsecRound, Utc};
    use flate2::read::GzEncoder;
    use futures::channel::mpsc;
    use pretty_assertions::assert_eq;
    use shared::assert_event_data_eq;
    use std::{
        io::{Cursor, Read},
        net::SocketAddr,
    };

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<AwsKinesisFirehoseConfig>();
    }

    async fn source(
        access_key: Option<String>,
        record_compression: Option<Compression>,
    ) -> (mpsc::Receiver<Event>, SocketAddr) {
        let (sender, recv) = Pipeline::new_test();
        let address = next_addr();
        tokio::spawn(async move {
            AwsKinesisFirehoseConfig {
                address,
                tls: None,
                access_key,
                record_compression,
            }
            .build(SourceContext::new_test(sender))
            .await
            .unwrap()
            .await
            .unwrap()
        });
        wait_for_tcp(address).await;
        (recv, address)
    }

    /// Sends the body to the address with the appropriate Firehose headers
    ///
    /// https://docs.aws.amazon.com/firehose/latest/dev/httpdeliveryrequestresponse.html
    async fn send(
        address: SocketAddr,
        timestamp: DateTime<Utc>,
        records: Vec<&[u8]>,
        key: Option<&str>,
        request_id: &str,
        source_arn: &str,
        gzip: bool,
        record_compression: Compression,
    ) -> reqwest::Result<reqwest::Response> {
        let request = models::FirehoseRequest {
            request_id: request_id.to_string(),
            timestamp,
            records: records
                .into_iter()
                .map(|record| models::EncodedFirehoseRecord {
                    data: encode_record(record, record_compression).unwrap(),
                })
                .collect(),
        };

        let mut builder = reqwest::Client::new()
            .post(&format!("http://{}", address))
            .header("host", address.to_string())
            .header(
                "x-amzn-trace-id",
                "Root=1-5f5fbf1c-877c68cace58bea222ddbeec",
            )
            .header("x-amz-firehose-protocol-version", "1.0")
            .header("x-amz-firehose-request-id", request_id.to_string())
            .header("x-amz-firehose-source-arn", source_arn.to_string())
            .header("user-agent", "Amazon Kinesis Data Firehose Agent/1.0")
            .header("content-type", "application/json");

        if let Some(key) = key {
            builder = builder.header("x-amz-firehose-access-key", key);
        }

        if gzip {
            let mut gz = GzEncoder::new(
                Cursor::new(serde_json::to_vec(&request).unwrap()),
                flate2::Compression::fast(),
            );
            let mut buffer = Vec::new();
            gz.read_to_end(&mut buffer).unwrap();
            builder = builder.header("content-encoding", "gzip").body(buffer);
        } else {
            builder = builder.json(&request);
        }

        builder.send().await
    }

    /// Encodes record data to mach AWS's representation: base64 encoded with an additional
    /// compression
    fn encode_record(record: &[u8], compression: Compression) -> std::io::Result<String> {
        let compressed = match compression {
            Compression::Auto => panic!("cannot encode records as Auto"),
            Compression::Gzip => {
                let mut buffer = Vec::new();
                if !record.is_empty() {
                    let mut gz = GzEncoder::new(record, flate2::Compression::fast());
                    gz.read_to_end(&mut buffer)?;
                }
                buffer
            }
            Compression::None => record.to_vec(),
        };

        Ok(base64::encode(&compressed))
    }

    #[tokio::test]
    async fn aws_kinesis_firehose_forwards_events() {
        // example CloudWatch Logs subscription event
        let record = r#"
{
  "messageType": "DATA_MESSAGE",
  "owner": "071959437513",
  "logGroup": "/jesse/test",
  "logStream": "test",
  "subscriptionFilters": [
    "Destination"
  ],
  "logEvents": [
    {
      "id": "35683658089614582423604394983260738922885519999578275840",
      "timestamp": 1600110569039,
      "message": "{\"bytes\":26780,\"datetime\":\"14/Sep/2020:11:45:41 -0400\",\"host\":\"157.130.216.193\",\"method\":\"PUT\",\"protocol\":\"HTTP/1.0\",\"referer\":\"https://www.principalcross-platform.io/markets/ubiquitous\",\"request\":\"/expedite/convergence\",\"source_type\":\"stdin\",\"status\":301,\"user-identifier\":\"-\"}"
    },
    {
      "id": "35683658089659183914001456229543810359430816722590236673",
      "timestamp": 1600110569041,
      "message": "{\"bytes\":17707,\"datetime\":\"14/Sep/2020:11:45:41 -0400\",\"host\":\"109.81.244.252\",\"method\":\"GET\",\"protocol\":\"HTTP/2.0\",\"referer\":\"http://www.investormission-critical.io/24/7/vortals\",\"request\":\"/scale/functionalities/optimize\",\"source_type\":\"stdin\",\"status\":502,\"user-identifier\":\"feeney1708\"}"
    }
  ]
}
"#.as_bytes();

        let gziped_record = {
            let mut buf = Vec::new();
            let mut gz = GzEncoder::new(record, flate2::Compression::fast());
            gz.read_to_end(&mut buf).unwrap();
            buf
        };

        let cases = vec![
            (
                Compression::Auto,
                Compression::Gzip,
                true,
                record.to_owned(),
                record.to_owned(),
            ),
            (
                Compression::Auto,
                Compression::None,
                true,
                record.to_owned(),
                record.to_owned(),
            ),
            (
                Compression::None,
                Compression::Gzip,
                true,
                record.to_owned(),
                gziped_record,
            ),
            (
                Compression::None,
                Compression::None,
                true,
                record.to_owned(),
                record.to_owned(),
            ),
            (
                Compression::Gzip,
                Compression::Gzip,
                true,
                record.to_owned(),
                record.to_owned(),
            ),
            (
                Compression::Gzip,
                Compression::None,
                false,
                record.to_owned(),
                record.to_owned(),
            ),
            (
                Compression::Gzip,
                Compression::Gzip,
                true,
                Vec::new(),
                Vec::new(),
            ),
        ];

        for (source_record_compression, record_compression, success, record, expected) in cases {
            println!(
                "test case: ({}, {})",
                &source_record_compression, &record_compression
            );

            let (rx, addr) = source(None, Some(source_record_compression)).await;

            let source_arn = "arn:aws:firehose:us-east-1:111111111111:deliverystream/test";
            let request_id = "e17265d6-97af-4938-982e-90d5614c4242";
            let timestamp: DateTime<Utc> = Utc::now();

            let res = send(
                addr,
                timestamp,
                vec![&record],
                None,
                request_id,
                source_arn,
                false,
                record_compression,
            )
            .await
            .unwrap();

            if success {
                assert_eq!(200, res.status().as_u16());

                let events = collect_ready(rx).await;
                assert_event_data_eq!(
                    events,
                    vec![log_event! {
                        "timestamp" => timestamp.trunc_subsecs(3), // AWS sends timestamps as ms
                        "message"=> Bytes::from(expected),
                        "request_id" => request_id,
                        "source_arn" => source_arn,
                    },]
                );

                let response: models::FirehoseResponse = res.json().await.unwrap();
                assert_eq!(response.request_id, request_id);
            } else {
                assert_eq!(400, res.status().as_u16());
            }
        }
    }

    #[tokio::test]
    async fn aws_kinesis_firehose_forwards_events_gzip_request() {
        // example CloudWatch Logs subscription event
        let record = r#"
{
  "messageType": "DATA_MESSAGE",
  "owner": "071959437513",
  "logGroup": "/jesse/test",
  "logStream": "test",
  "subscriptionFilters": [
    "Destination"
  ],
  "logEvents": [
    {
      "id": "35683658089614582423604394983260738922885519999578275840",
      "timestamp": 1600110569039,
      "message": "{\"bytes\":26780,\"datetime\":\"14/Sep/2020:11:45:41 -0400\",\"host\":\"157.130.216.193\",\"method\":\"PUT\",\"protocol\":\"HTTP/1.0\",\"referer\":\"https://www.principalcross-platform.io/markets/ubiquitous\",\"request\":\"/expedite/convergence\",\"source_type\":\"stdin\",\"status\":301,\"user-identifier\":\"-\"}"
    },
    {
      "id": "35683658089659183914001456229543810359430816722590236673",
      "timestamp": 1600110569041,
      "message": "{\"bytes\":17707,\"datetime\":\"14/Sep/2020:11:45:41 -0400\",\"host\":\"109.81.244.252\",\"method\":\"GET\",\"protocol\":\"HTTP/2.0\",\"referer\":\"http://www.investormission-critical.io/24/7/vortals\",\"request\":\"/scale/functionalities/optimize\",\"source_type\":\"stdin\",\"status\":502,\"user-identifier\":\"feeney1708\"}"
    }
  ]
}
"#;

        let (rx, addr) = source(None, None).await;

        let source_arn = "arn:aws:firehose:us-east-1:111111111111:deliverystream/test";
        let request_id = "e17265d6-97af-4938-982e-90d5614c4242";
        let timestamp: DateTime<Utc> = Utc::now();

        let res = send(
            addr,
            timestamp,
            vec![record.as_bytes()],
            None,
            request_id,
            source_arn,
            true,
            Compression::None,
        )
        .await
        .unwrap();
        assert_eq!(200, res.status().as_u16());

        let events = collect_ready(rx).await;
        assert_event_data_eq!(
            events,
            vec![log_event! {
                "timestamp" => timestamp.trunc_subsecs(3), // AWS sends timestamps as ms
                "message"=> record,
                "request_id" => request_id,
                "source_arn" => source_arn,
            },]
        );

        let response: models::FirehoseResponse = res.json().await.unwrap();
        assert_eq!(response.request_id, request_id);
    }

    #[tokio::test]
    async fn aws_kinesis_firehose_rejects_bad_access_key() {
        let (_rx, addr) = source(Some("an access key".to_string()), None).await;

        let request_id = "e17265d6-97af-4938-982e-90d5614c4242";

        let res = send(
            addr,
            Utc::now(),
            vec![],
            Some("bad access key"),
            request_id,
            "",
            false,
            Compression::None,
        )
        .await
        .unwrap();
        assert_eq!(401, res.status().as_u16());

        let response: models::FirehoseResponse = res.json().await.unwrap();
        assert_eq!(response.request_id, request_id);
    }
}
