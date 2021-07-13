//! An instrumenting state wrapper.

use crate::internal_events::kubernetes::instrumenting_state as internal_events;
use async_trait::async_trait;
use futures::future::BoxFuture;

/// A [`super::Write`] implementation that wraps another [`super::Write`] and
/// adds instrumentation.
pub struct Writer<T> {
    inner: T,
}

impl<T> Writer<T> {
    /// Take a [`super::Write`] and return it wrapped with [`Writer`].
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<T> super::Write for Writer<T>
where
    T: super::Write + Send,
{
    type Item = <T as super::Write>::Item;

    async fn add(&mut self, item: Self::Item) {
        emit!(internal_events::StateItemAdded);
        self.inner.add(item).await
    }

    async fn update(&mut self, item: Self::Item) {
        emit!(internal_events::StateItemUpdated);
        self.inner.update(item).await
    }

    async fn delete(&mut self, item: Self::Item) {
        emit!(internal_events::StateItemDeleted);
        self.inner.delete(item).await
    }

    async fn resync(&mut self) {
        emit!(internal_events::StateResynced);
        self.inner.resync().await
    }
}

#[async_trait]
impl<T> super::MaintainedWrite for Writer<T>
where
    T: super::MaintainedWrite + Send,
{
    fn maintenance_request(&mut self) -> Option<BoxFuture<'_, ()>> {
        self.inner.maintenance_request().map(|future| {
            emit!(internal_events::StateMaintenanceRequested);
            future
        })
    }

    async fn perform_maintenance(&mut self) {
        emit!(internal_events::StateMaintenancePerformed);
        self.inner.perform_maintenance().await
    }
}

#[cfg(test)]
mod tests {
    use super::super::{mock, MaintainedWrite, Write};
    use super::*;
    use crate::{event::metric::MetricValue, test_util::trace_init};
    use futures::{channel::mpsc, SinkExt, StreamExt};
    use k8s_openapi::{api::core::v1::Pod, apimachinery::pkg::apis::meta::v1::ObjectMeta};
    use once_cell::sync::OnceCell;
    use tokio::sync::{Mutex, MutexGuard};

    fn prepare_test() -> (
        Writer<mock::Writer<Pod>>,
        mpsc::Receiver<mock::ScenarioEvent<Pod>>,
        mpsc::Sender<()>,
    ) {
        let (events_tx, events_rx) = mpsc::channel(0);
        let (actions_tx, actions_rx) = mpsc::channel(0);
        let writer = mock::Writer::new(events_tx, actions_rx);
        let writer = Writer::new(writer);
        (writer, events_rx, actions_tx)
    }

    fn make_pod() -> Pod {
        Pod {
            metadata: ObjectMeta {
                name: Some("pod_name".to_owned()),
                uid: Some("pod_uid".to_owned()),
                ..ObjectMeta::default()
            },
            ..Pod::default()
        }
    }

    fn get_metric_value(op_kind: &'static str) -> Option<MetricValue> {
        let controller = crate::metrics::get_controller().expect("failed to init metric container");

        let tags_to_lookup = Some(
            vec![("op_kind".to_owned(), op_kind.to_owned())]
                .into_iter()
                .collect(),
        );

        crate::metrics::capture_metrics(controller)
            .find(|metric| {
                metric.name() == "k8s_state_ops_total" && metric.tags() == tags_to_lookup.as_ref()
            })
            .map(|metric| metric.value().clone())
    }

    fn assert_counter_changed(
        before: Option<MetricValue>,
        after: Option<MetricValue>,
        expected_difference: u64,
    ) {
        let before = before.unwrap_or(MetricValue::Counter { value: 0.0 });
        let after = after.unwrap_or(MetricValue::Counter { value: 0.0 });

        let (before, after) = match (before, after) {
            (MetricValue::Counter { value: before }, MetricValue::Counter { value: after }) => {
                (before, after)
            }
            _ => panic!("Metrics kind mismatch"),
        };

        let difference = after - before;

        assert_eq!(difference, expected_difference as f64);
    }

    /// Guarantees only one test will run at a time.
    /// This is required because we assert on a global state, and we don't
    /// want interference.
    async fn tests_lock() -> MutexGuard<'static, ()> {
        static INSTANCE: OnceCell<Mutex<()>> = OnceCell::new();
        INSTANCE.get_or_init(|| Mutex::new(())).lock().await
    }

    #[tokio::test]
    async fn add() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (mut writer, mut events_rx, mut actions_tx) = prepare_test();

        let pod = make_pod();

        let join = {
            let pod = pod.clone();
            let before = get_metric_value("item_added");
            tokio::spawn(async move {
                assert_eq!(
                    events_rx.next().await.unwrap().unwrap_op(),
                    (pod, mock::OpKind::Add)
                );

                // By now metrics should've updated.
                let after = get_metric_value("item_added");
                assert_counter_changed(before, after, 1);

                actions_tx.send(()).await.unwrap();
            })
        };

        writer.add(pod).await;
        join.await.unwrap();
    }

    #[tokio::test]
    async fn update() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (mut writer, mut events_rx, mut actions_tx) = prepare_test();

        let pod = make_pod();

        let join = {
            let pod = pod.clone();
            let before = get_metric_value("item_updated");
            tokio::spawn(async move {
                assert_eq!(
                    events_rx.next().await.unwrap().unwrap_op(),
                    (pod, mock::OpKind::Update)
                );

                // By now metrics should've updated.
                let after = get_metric_value("item_updated");
                assert_counter_changed(before, after, 1);

                actions_tx.send(()).await.unwrap();
            })
        };

        writer.update(pod).await;
        join.await.unwrap();
    }

    #[tokio::test]
    async fn delete() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (mut writer, mut events_rx, mut actions_tx) = prepare_test();

        let pod = make_pod();

        let join = {
            let pod = pod.clone();
            let before = get_metric_value("item_deleted");
            tokio::spawn(async move {
                assert_eq!(
                    events_rx.next().await.unwrap().unwrap_op(),
                    (pod, mock::OpKind::Delete)
                );

                // By now metrics should've updated.
                let after = get_metric_value("item_deleted");
                assert_counter_changed(before, after, 1);

                actions_tx.send(()).await.unwrap();
            })
        };

        writer.delete(pod).await;
        join.await.unwrap();
    }

    #[tokio::test]
    async fn resync() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (mut writer, mut events_rx, mut actions_tx) = prepare_test();

        let join = {
            let before = get_metric_value("resynced");
            tokio::spawn(async move {
                assert!(matches!(
                    events_rx.next().await.unwrap(),
                    mock::ScenarioEvent::Resync
                ));

                let after = get_metric_value("resynced");
                assert_counter_changed(before, after, 1);

                actions_tx.send(()).await.unwrap();
            })
        };

        writer.resync().await;
        join.await.unwrap();
    }

    #[tokio::test]
    async fn request_maintenance_without_maintenance() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (mut writer, _events_rx, _actions_tx) = prepare_test();
        let before = get_metric_value("maintenance_requested");
        let _ = writer.maintenance_request();
        let after = get_metric_value("maintenance_requested");
        assert_counter_changed(before, after, 0);
    }

    #[tokio::test]
    async fn request_maintenance_with_maintenance() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (events_tx, _events_rx) = mpsc::channel(0);
        let (_actions_tx, actions_rx) = mpsc::channel(0);
        let (maintenance_request_events_tx, _maintenance_request_events_rx) = mpsc::channel(0);
        let (_maintenance_request_actions_tx, maintenance_request_actions_rx) = mpsc::channel(0);
        let writer = mock::Writer::<Pod>::new_with_maintenance(
            events_tx,
            actions_rx,
            maintenance_request_events_tx,
            maintenance_request_actions_rx,
        );
        let mut writer = Writer::new(writer);
        let before = get_metric_value("maintenance_requested");
        let _ = writer.maintenance_request();
        let after = get_metric_value("maintenance_requested");
        assert_counter_changed(before, after, 1);
    }

    #[tokio::test]
    async fn perform_maintenance() {
        trace_init();
        let _ = crate::metrics::init();
        let _guard = tests_lock().await;

        let (mut writer, mut events_rx, mut actions_tx) = prepare_test();

        let join = {
            let before = get_metric_value("maintenance_performed");
            tokio::spawn(async move {
                assert!(matches!(
                    events_rx.next().await.unwrap(),
                    mock::ScenarioEvent::Maintenance
                ));

                let after = get_metric_value("maintenance_performed");
                assert_counter_changed(before, after, 1);

                actions_tx.send(()).await.unwrap();
            })
        };

        writer.perform_maintenance().await;
        join.await.unwrap();
    }
}
