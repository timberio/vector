use crate::{
    config::{DataType, SourceConfig, SourceContext, SourceDescription},
    metrics::Controller,
    metrics::{capture_metrics, get_controller},
    shutdown::ShutdownSignal,
    Pipeline,
};
use futures::{stream, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time;
use tokio_stream::wrappers::IntervalStream;

#[derive(Deserialize, Serialize, Debug, Clone, Derivative)]
#[derivative(Default)]
#[serde(deny_unknown_fields, default)]
pub struct InternalMetricsConfig {
    #[derivative(Default(value = "2"))]
    scrape_interval_secs: u64,
}

inventory::submit! {
    SourceDescription::new::<InternalMetricsConfig>("internal_metrics")
}

impl_generate_config_from_default!(InternalMetricsConfig);

#[async_trait::async_trait]
#[typetag::serde(name = "internal_metrics")]
impl SourceConfig for InternalMetricsConfig {
    async fn build(&self, cx: SourceContext) -> crate::Result<super::Source> {
        if self.scrape_interval_secs == 0 {
            warn!(
                "Interval set to 0 secs, this could result in high CPU utilization. It is suggested to use interval >= 1 secs.",
            );
        }
        let interval = time::Duration::from_secs(self.scrape_interval_secs);
        Ok(Box::pin(run(
            get_controller()?,
            interval,
            cx.out,
            cx.shutdown,
        )))
    }

    fn output_type(&self) -> DataType {
        DataType::Metric
    }

    fn source_type(&self) -> &'static str {
        "internal_metrics"
    }
}

async fn run(
    controller: &Controller,
    interval: time::Duration,
    out: Pipeline,
    shutdown: ShutdownSignal,
) -> Result<(), ()> {
    let mut out =
        out.sink_map_err(|error| error!(message = "Error sending internal metrics.", %error));

    let mut interval = IntervalStream::new(time::interval(interval)).take_until(shutdown);
    while interval.next().await.is_some() {
        let hostname = crate::get_hostname();

        let metrics = capture_metrics(controller);

        out.send_all(&mut stream::iter(metrics.map(|mut metric| {
            if let Ok(hostname) = &hostname {
                metric.insert_tag("host".into(), hostname.into());
            }
            Ok(metric.into())
        })))
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::event::metric::{Metric, MetricValue};
    use crate::metrics::{capture_metrics, get_controller};
    use metrics::{counter, gauge, histogram};
    use std::collections::BTreeMap;

    #[test]
    fn generate_config() {
        crate::test_util::test_generate_config::<super::InternalMetricsConfig>();
    }

    #[test]
    fn captures_internal_metrics() {
        let _ = crate::metrics::init();

        // There *seems* to be a race condition here (CI was flaky), so add a slight delay.
        std::thread::sleep(std::time::Duration::from_millis(300));

        gauge!("foo", 1.0);
        gauge!("foo", 2.0);
        counter!("bar", 3);
        counter!("bar", 4);
        histogram!("baz", 5.0);
        histogram!("baz", 6.0);
        histogram!("quux", 8.0, "host" => "foo");
        histogram!("quux", 8.1, "host" => "foo");

        let controller = get_controller().expect("no controller");

        // There *seems* to be a race condition here (CI was flaky), so add a slight delay.
        std::thread::sleep(std::time::Duration::from_millis(300));

        let output = capture_metrics(controller)
            .map(|metric| (metric.name().to_string(), metric))
            .collect::<BTreeMap<String, Metric>>();

        assert_eq!(&MetricValue::Gauge { value: 2.0 }, output["foo"].value());
        assert_eq!(&MetricValue::Counter { value: 7.0 }, output["bar"].value());

        match &output["baz"].value() {
            MetricValue::AggregatedHistogram {
                buckets,
                count,
                sum,
            } => {
                // This index is _only_ stable so long as the offsets in
                // [`metrics::handle::Histogram::new`] are hard-coded. If this
                // check fails you might look there and see if we've allowed
                // users to set their own bucket widths.
                assert_eq!(buckets[11].count, 2);
                assert_eq!(*count, 2);
                assert_eq!(*sum, 11.0);
            }
            _ => panic!("wrong type"),
        }

        match &output["quux"].value() {
            MetricValue::AggregatedHistogram {
                buckets,
                count,
                sum,
            } => {
                // This index is _only_ stable so long as the offsets in
                // [`metrics::handle::Histogram::new`] are hard-coded. If this
                // check fails you might look there and see if we've allowed
                // users to set their own bucket widths.
                assert_eq!(buckets[11].count, 1);
                assert_eq!(buckets[12].count, 1);
                assert_eq!(*count, 2);
                assert_eq!(*sum, 16.1);
            }
            _ => panic!("wrong type"),
        }

        let mut labels = BTreeMap::new();
        labels.insert(String::from("host"), String::from("foo"));
        assert_eq!(Some(&labels), output["quux"].tags());
    }
}
