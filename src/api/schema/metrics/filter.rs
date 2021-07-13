use super::{EventsInTotal, EventsOutTotal, ProcessedBytesTotal, ProcessedEventsTotal};
use crate::{
    event::{Metric, MetricValue},
    metrics::{capture_metrics, get_controller, Controller},
};
use async_stream::stream;
use lazy_static::lazy_static;
use std::{collections::BTreeMap, sync::Arc};
use tokio::time::Duration;
use tokio_stream::{Stream, StreamExt};

lazy_static! {
    static ref GLOBAL_CONTROLLER: Arc<&'static Controller> =
        Arc::new(get_controller().expect("Metrics system not initialized. Please report."));
}

/// Sums an iteratable of `&Metric`, by folding metric values. Convenience function typically
/// used to get aggregate metrics.
fn sum_metrics<'a, I: IntoIterator<Item = &'a Metric>>(metrics: I) -> Option<Metric> {
    let mut iter = metrics.into_iter();
    let m = iter.next()?;

    Some(iter.fold(
        m.clone(),
        |mut m1, m2| {
            if m1.update(&m2) {
                m1
            } else {
                m2.clone()
            }
        },
    ))
}

/// Sums an iteratable of `Metric`, by folding metric values. Convenience function typically
/// used to get aggregate metrics.
fn sum_metrics_owned<I: IntoIterator<Item = Metric>>(metrics: I) -> Option<Metric> {
    let mut iter = metrics.into_iter();
    let m = iter.next()?;

    Some(iter.fold(m, |mut m1, m2| if m1.update(&m2) { m1 } else { m2 }))
}

pub trait MetricsFilter<'a> {
    fn processed_events_total(&self) -> Option<ProcessedEventsTotal>;
    fn processed_bytes_total(&self) -> Option<ProcessedBytesTotal>;
    fn events_in_total(&self) -> Option<EventsInTotal>;
    fn events_out_total(&self) -> Option<EventsOutTotal>;
}

impl<'a> MetricsFilter<'a> for Vec<Metric> {
    fn processed_events_total(&self) -> Option<ProcessedEventsTotal> {
        let sum = sum_metrics(self.iter().filter(|m| m.name() == "processed_events_total"))?;

        Some(ProcessedEventsTotal::new(sum))
    }

    fn processed_bytes_total(&self) -> Option<ProcessedBytesTotal> {
        let sum = sum_metrics(self.iter().filter(|m| m.name() == "processed_bytes_total"))?;

        Some(ProcessedBytesTotal::new(sum))
    }

    fn events_in_total(&self) -> Option<EventsInTotal> {
        let sum = sum_metrics(self.iter().filter(|m| m.name() == "events_in_total"))?;

        Some(EventsInTotal::new(sum))
    }

    fn events_out_total(&self) -> Option<EventsOutTotal> {
        let sum = sum_metrics(self.iter().filter(|m| m.name() == "events_out_total"))?;

        Some(EventsOutTotal::new(sum))
    }
}

impl<'a> MetricsFilter<'a> for Vec<&'a Metric> {
    fn processed_events_total(&self) -> Option<ProcessedEventsTotal> {
        let sum = sum_metrics(
            self.iter()
                .filter(|m| m.name() == "processed_events_total")
                .copied(),
        )?;

        Some(ProcessedEventsTotal::new(sum))
    }

    fn processed_bytes_total(&self) -> Option<ProcessedBytesTotal> {
        let sum = sum_metrics(
            self.iter()
                .filter(|m| m.name() == "processed_bytes_total")
                .copied(),
        )?;

        Some(ProcessedBytesTotal::new(sum))
    }

    fn events_in_total(&self) -> Option<EventsInTotal> {
        let sum = sum_metrics(
            self.iter()
                .filter(|m| m.name() == "events_in_total")
                .copied(),
        )?;

        Some(EventsInTotal::new(sum))
    }

    fn events_out_total(&self) -> Option<EventsOutTotal> {
        let sum = sum_metrics(
            self.iter()
                .filter(|m| m.name() == "events_out_total")
                .copied(),
        )?;

        Some(EventsOutTotal::new(sum))
    }
}

/// Returns a stream of `Metric`s, collected at the provided millisecond interval.
pub fn get_metrics(interval: i32) -> impl Stream<Item = Metric> {
    let controller = get_controller().unwrap();
    let mut interval = tokio::time::interval(Duration::from_millis(interval as u64));

    stream! {
        loop {
            interval.tick().await;
            for m in capture_metrics(controller) {
                yield m;
            }
        }
    }
}

pub fn get_all_metrics(interval: i32) -> impl Stream<Item = Vec<Metric>> {
    let controller = get_controller().unwrap();
    let mut interval = tokio::time::interval(Duration::from_millis(interval as u64));

    stream! {
        loop {
            interval.tick().await;
            yield capture_metrics(controller).collect()
        }
    }
}

/// Return Vec<Metric> based on a component name tag.
pub fn by_component_name(component_name: &str) -> Vec<Metric> {
    capture_metrics(&GLOBAL_CONTROLLER)
        .filter_map(|m| m.tag_matches("component_name", component_name).then(|| m))
        .collect()
}

type MetricFilterFn = dyn Fn(&Metric) -> bool + Send + Sync;

/// Returns a stream of `Vec<Metric>`, where `metric_name` matches the name of the metric
/// (e.g. "processed_events_total"), and the value is derived from `MetricValue::Counter`. Uses a
/// local cache to match against the `component_name` of a metric, to return results only when
/// the value of a current iteration is greater than the previous. This is useful for the client
/// to be notified as metrics increase without returning 'empty' or identical results.
pub fn component_counter_metrics(
    interval: i32,
    filter_fn: &'static MetricFilterFn,
) -> impl Stream<Item = Vec<Metric>> {
    let mut cache = BTreeMap::new();

    get_all_metrics(interval).map(move |m| {
        m.into_iter()
            .filter(filter_fn)
            .filter_map(|m| m.tag_value("component_name").map(|name| (name, m)))
            .fold(BTreeMap::new(), |mut map, (name, m)| {
                map.entry(name).or_insert_with(Vec::new).push(m);
                map
            })
            .into_iter()
            .filter_map(|(name, metrics)| {
                let m = sum_metrics_owned(metrics)?;
                match m.value() {
                    MetricValue::Counter { value }
                        if cache.insert(name, *value).unwrap_or(0.00) < *value =>
                    {
                        Some(m)
                    }
                    _ => None,
                }
            })
            .collect()
    })
}

/// Returns the throughput of a 'counter' metric, sampled over `interval` millseconds
/// and filtered by the provided `filter_fn`.
pub fn counter_throughput(
    interval: i32,
    filter_fn: &'static MetricFilterFn,
) -> impl Stream<Item = (Metric, f64)> {
    let mut last = 0.00;

    get_metrics(interval)
        .filter(filter_fn)
        .filter_map(move |m| match m.value() {
            MetricValue::Counter { value } if *value > last => {
                let throughput = value - last;
                last = *value;
                Some((m, throughput))
            }
            _ => None,
        })
        // Ignore the first, since we only care about sampling between `interval`
        .skip(1)
}

/// Returns the throughput of a 'counter' metric, sampled over `interval` milliseconds
/// and filtered by the provided `filter_fn`, aggregated against each component.
pub fn component_counter_throughputs(
    interval: i32,
    filter_fn: &'static MetricFilterFn,
) -> impl Stream<Item = Vec<(Metric, f64)>> {
    let mut cache = BTreeMap::new();

    get_all_metrics(interval)
        .map(move |m| {
            m.into_iter()
                .filter(filter_fn)
                .filter_map(|m| m.tag_value("component_name").map(|name| (name, m)))
                .fold(BTreeMap::new(), |mut map, (name, m)| {
                    map.entry(name).or_insert_with(Vec::new).push(m);
                    map
                })
                .into_iter()
                .filter_map(|(name, metrics)| {
                    let m = sum_metrics_owned(metrics)?;
                    match m.value() {
                        MetricValue::Counter { value } => {
                            let last = cache.insert(name, *value).unwrap_or(0.00);
                            let throughput = value - last;
                            Some((m, throughput))
                        }
                        _ => None,
                    }
                })
                .collect()
        })
        // Ignore the first, since we only care about sampling between `interval`
        .skip(1)
}
