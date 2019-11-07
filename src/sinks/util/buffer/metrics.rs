use crate::event::{Event, Metric};
use crate::sinks::util::Batch;
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::hash::{Hash, Hasher};

#[derive(Clone)]
struct MetricEntry(Metric);

impl Eq for MetricEntry {}

impl Hash for MetricEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(&self.0).hash(state);

        match &self.0 {
            Metric::Counter { name, .. } | Metric::AggregatedCounter { name, .. } => {
                name.hash(state);
            }
            Metric::Gauge { name, .. } | Metric::AggregatedGauge { name, .. } => {
                name.hash(state);
            }
            Metric::Set { name, val, .. } => {
                name.hash(state);
                val.hash(state);
            }
            Metric::Histogram { name, val, .. } => {
                name.hash(state);
                val.to_bits().hash(state);
            }
            Metric::AggregatedHistogram { name, buckets, .. } => {
                name.hash(state);
                for bucket in buckets {
                    bucket.to_bits().hash(state);
                }
            }
            Metric::AggregatedSummary {
                name, quantiles, ..
            } => {
                name.hash(state);
                for quantile in quantiles {
                    quantile.to_bits().hash(state);
                }
            }
        }

        self.0
            .tags()
            .as_ref()
            .map(|ts| ts.iter().for_each(|t| t.hash(state)));
    }
}

impl PartialEq for MetricEntry {
    fn eq(&self, other: &Self) -> bool {
        let mut state = DefaultHasher::new();
        self.hash(&mut state);
        let hash1 = state.finish();

        let mut state = DefaultHasher::new();
        other.hash(&mut state);
        let hash2 = state.finish();

        hash1 == hash2
    }
}

#[derive(Clone, PartialEq)]
pub struct MetricBuffer {
    metrics: HashSet<MetricEntry>,
}

impl MetricBuffer {
    pub fn new() -> Self {
        Self {
            metrics: HashSet::new(),
        }
    }
}

impl Batch for MetricBuffer {
    type Input = Event;
    type Output = Vec<Metric>;

    fn len(&self) -> usize {
        self.num_items()
    }

    fn push(&mut self, item: Self::Input) {
        let item = item.into_metric();
        let new = MetricEntry(item.clone());

        match item {
            // cannot be aggregated
            Metric::Set { .. }
            | Metric::AggregatedGauge { .. }
            | Metric::AggregatedSummary { .. } => {
                self.metrics.insert(new);
            }
            _ => {
                // counters, gauges, aggregated counters, histograms
                if let Some(MetricEntry(mut existing)) = self.metrics.take(&new) {
                    existing.merge(&item);
                    self.metrics.insert(MetricEntry(existing));
                } else {
                    self.metrics.insert(new);
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.num_items() == 0
    }

    fn fresh(&self) -> Self {
        Self {
            metrics: HashSet::new(),
        }
    }

    fn finish(self) -> Self::Output {
        self.metrics.into_iter().map(|e| e.0).collect()
    }

    fn num_items(&self) -> usize {
        self.metrics.len()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::sinks::util::batch::BatchSink;
    use crate::{event::metric::Metric, Event};
    use futures::{future::Future, stream, Sink};
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::time::Duration;

    fn tag(name: &str) -> HashMap<String, String> {
        vec![(name.to_owned(), "true".to_owned())]
            .into_iter()
            .collect()
    }

    fn sorted(buffer: &Vec<Metric>) -> Vec<Metric> {
        let mut buffer = buffer.clone();
        buffer.sort_by_key(|k| format!("{:?}", k));
        buffer
    }

    #[test]
    fn metric_buffer_counters() {
        let sink = BatchSink::new_max(vec![], MetricBuffer::new(), 6, Some(Duration::from_secs(1)));

        let mut events = Vec::new();
        for i in 0..4 {
            let event = Event::Metric(Metric::Counter {
                name: "counter-0".into(),
                val: i as f64,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        for i in 0..4 {
            let event = Event::Metric(Metric::Counter {
                name: format!("counter-{}", i),
                val: i as f64,
                timestamp: None,
                tags: Some(tag("staging")),
            });
            events.push(event);
        }

        for i in 0..4 {
            let event = Event::Metric(Metric::Counter {
                name: format!("counter-{}", i),
                val: i as f64,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        let (buffer, _) = sink
            .send_all(stream::iter_ok(events.into_iter()))
            .wait()
            .unwrap();

        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer[0].len(), 6);
        assert_eq!(buffer[1].len(), 2);

        assert_eq!(
            sorted(&buffer[0].clone().finish()),
            [
                Metric::Counter {
                    name: "counter-0".into(),
                    val: 0.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Counter {
                    name: "counter-0".into(),
                    val: 6.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Counter {
                    name: "counter-1".into(),
                    val: 1.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Counter {
                    name: "counter-1".into(),
                    val: 1.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Counter {
                    name: "counter-2".into(),
                    val: 2.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Counter {
                    name: "counter-3".into(),
                    val: 3.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
            ]
        );

        assert_eq!(
            sorted(&buffer[1].clone().finish()),
            [
                Metric::Counter {
                    name: "counter-2".into(),
                    val: 2.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Counter {
                    name: "counter-3".into(),
                    val: 3.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
            ]
        );
    }

    #[test]
    fn metric_buffer_gauges() {
        let sink = BatchSink::new_max(vec![], MetricBuffer::new(), 4, Some(Duration::from_secs(1)));

        let mut events = Vec::new();
        for i in 0..4 {
            let event = Event::Metric(Metric::Gauge {
                name: "gauge-0".into(),
                val: i as f64,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        for i in 0..5 {
            let event = Event::Metric(Metric::Gauge {
                name: format!("gauge-{}", i),
                val: i as f64,
                timestamp: None,
                tags: Some(tag("staging")),
            });
            events.push(event);
        }

        for i in 0..5 {
            let event = Event::Metric(Metric::Gauge {
                name: format!("gauge-{}", i),
                val: i as f64,
                timestamp: None,
                tags: Some(tag("staging")),
            });
            events.push(event);
        }

        let (buffer, _) = sink
            .send_all(stream::iter_ok(events.into_iter()))
            .wait()
            .unwrap();

        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer[0].len(), 4);
        assert_eq!(buffer[1].len(), 4);
        assert_eq!(buffer[2].len(), 3);

        assert_eq!(
            sorted(&buffer[0].clone().finish()),
            [
                Metric::Gauge {
                    name: "gauge-0".into(),
                    val: 0.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-0".into(),
                    val: 6.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Gauge {
                    name: "gauge-1".into(),
                    val: 1.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-2".into(),
                    val: 2.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
            ]
        );

        assert_eq!(
            sorted(&buffer[1].clone().finish()),
            [
                Metric::Gauge {
                    name: "gauge-0".into(),
                    val: 0.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-1".into(),
                    val: 1.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-3".into(),
                    val: 3.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-4".into(),
                    val: 4.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
            ]
        );

        assert_eq!(
            sorted(&buffer[2].clone().finish()),
            [
                Metric::Gauge {
                    name: "gauge-2".into(),
                    val: 2.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-3".into(),
                    val: 3.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
                Metric::Gauge {
                    name: "gauge-4".into(),
                    val: 4.0,
                    timestamp: None,
                    tags: Some(tag("staging")),
                },
            ]
        );
    }

    #[test]
    fn metric_buffer_sets() {
        let sink = BatchSink::new_max(vec![], MetricBuffer::new(), 6, Some(Duration::from_secs(1)));

        let mut events = Vec::new();
        for i in 0..4 {
            let event = Event::Metric(Metric::Set {
                name: "set-0".into(),
                val: format!("{}", i),
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        for i in 0..4 {
            let event = Event::Metric(Metric::Set {
                name: "set-0".into(),
                val: format!("{}", i),
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        let (buffer, _) = sink
            .send_all(stream::iter_ok(events.into_iter()))
            .wait()
            .unwrap();

        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 1);

        assert_eq!(
            sorted(&buffer[0].clone().finish()),
            [
                Metric::Set {
                    name: "set-0".into(),
                    val: "0".into(),
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Set {
                    name: "set-0".into(),
                    val: "1".into(),
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Set {
                    name: "set-0".into(),
                    val: "2".into(),
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Set {
                    name: "set-0".into(),
                    val: "3".into(),
                    timestamp: None,
                    tags: Some(tag("production")),
                },
            ]
        );
    }

    #[test]
    fn metric_buffer_histograms() {
        let sink = BatchSink::new_max(vec![], MetricBuffer::new(), 6, Some(Duration::from_secs(1)));

        let mut events = Vec::new();
        for _i in 2..6 {
            let event = Event::Metric(Metric::Histogram {
                name: "hist-2".into(),
                val: 2.0,
                sample_rate: 10,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        for i in 2..6 {
            let event = Event::Metric(Metric::Histogram {
                name: format!("hist-{}", i),
                val: i as f64,
                sample_rate: 10,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        let (buffer, _) = sink
            .send_all(stream::iter_ok(events.into_iter()))
            .wait()
            .unwrap();

        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 1);

        assert_eq!(
            sorted(&buffer[0].clone().finish()),
            [
                Metric::Histogram {
                    name: "hist-2".into(),
                    val: 2.0,
                    sample_rate: 50,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Histogram {
                    name: "hist-3".into(),
                    val: 3.0,
                    sample_rate: 10,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Histogram {
                    name: "hist-4".into(),
                    val: 4.0,
                    sample_rate: 10,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::Histogram {
                    name: "hist-5".into(),
                    val: 5.0,
                    sample_rate: 10,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
            ]
        );
    }

    #[test]
    fn metric_buffer_aggregated_histograms() {
        let sink = BatchSink::new_max(vec![], MetricBuffer::new(), 6, Some(Duration::from_secs(1)));

        let mut events = Vec::new();
        for _i in 2..5 {
            let event = Event::Metric(Metric::AggregatedHistogram {
                name: "buckets-2".into(),
                buckets: vec![1.0, 2.0, 4.0],
                counts: vec![1, 2, 4],
                count: 6,
                sum: 10.0,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        for i in 2..5 {
            let event = Event::Metric(Metric::AggregatedHistogram {
                name: format!("buckets-{}", i),
                buckets: vec![1.0, 2.0, 4.0],
                counts: vec![1 * i, 2 * i, 4 * i],
                count: 6 * i,
                sum: 10.0,
                timestamp: None,
                tags: Some(tag("production")),
            });
            events.push(event);
        }

        let (buffer, _) = sink
            .send_all(stream::iter_ok(events.into_iter()))
            .wait()
            .unwrap();

        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 1);

        assert_eq!(
            sorted(&buffer[0].clone().finish()),
            [
                Metric::AggregatedHistogram {
                    name: "buckets-2".into(),
                    buckets: vec![1.0, 2.0, 4.0],
                    counts: vec![5, 10, 20],
                    count: 6 * 5,
                    sum: 40.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::AggregatedHistogram {
                    name: "buckets-3".into(),
                    buckets: vec![1.0, 2.0, 4.0],
                    counts: vec![3, 6, 12],
                    count: 6 * 3,
                    sum: 10.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::AggregatedHistogram {
                    name: "buckets-4".into(),
                    buckets: vec![1.0, 2.0, 4.0],
                    counts: vec![4, 8, 16],
                    count: 6 * 4,
                    sum: 10.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                }
            ]
        );
    }

    #[test]
    fn metric_buffer_aggregated_summaries() {
        let sink = BatchSink::new_max(vec![], MetricBuffer::new(), 6, Some(Duration::from_secs(1)));

        let mut events = Vec::new();
        for _ in 0..10 {
            for i in 2..5 {
                let event = Event::Metric(Metric::AggregatedSummary {
                    name: format!("quantiles-{}", i),
                    quantiles: vec![1.0, 2.0, 4.0],
                    values: vec![(1 * i) as f64, (2 * i) as f64, (4 * i) as f64],
                    count: 6 * i,
                    sum: 10.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                });
                events.push(event);
            }
        }

        let (buffer, _) = sink
            .send_all(stream::iter_ok(events.into_iter()))
            .wait()
            .unwrap();

        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 1);

        assert_eq!(
            sorted(&buffer[0].clone().finish()),
            [
                Metric::AggregatedSummary {
                    name: "quantiles-2".into(),
                    quantiles: vec![1.0, 2.0, 4.0],
                    values: vec![2.0, 4.0, 8.0],
                    count: 6 * 2,
                    sum: 10.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::AggregatedSummary {
                    name: "quantiles-3".into(),
                    quantiles: vec![1.0, 2.0, 4.0],
                    values: vec![3.0, 6.0, 12.0],
                    count: 6 * 3,
                    sum: 10.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                },
                Metric::AggregatedSummary {
                    name: "quantiles-4".into(),
                    quantiles: vec![1.0, 2.0, 4.0],
                    values: vec![4.0, 8.0, 16.0],
                    count: 6 * 4,
                    sum: 10.0,
                    timestamp: None,
                    tags: Some(tag("production")),
                }
            ]
        );
    }
}
