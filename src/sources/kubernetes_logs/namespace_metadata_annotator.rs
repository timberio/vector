//! Annotates events with namespace metadata.

#![deny(missing_docs)]

use crate::{
    event::{Event, LogEvent, PathComponent, PathIter},
    kubernetes as k8s,
};
use evmap::ReadHandle;
use k8s_openapi::{api::core::v1::Namespace, apimachinery::pkg::apis::meta::v1::ObjectMeta};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct FieldsSpec {
    pub namespace_labels: String,
}

impl Default for FieldsSpec {
    fn default() -> Self {
        Self {
            namespace_labels: "kubernetes.namespace_labels".to_owned(),
        }
    }
}

/// Annotate the event with namespace metadata.
pub struct NamespaceMetadataAnnotator {
    namespace_state_reader: ReadHandle<String, k8s::state::evmap::Value<Namespace>>,
    fields_spec: FieldsSpec,
}

impl NamespaceMetadataAnnotator {
    /// Create a new [`NamespaceMetadataAnnotator`].
    pub fn new(
        namespace_state_reader: ReadHandle<String, k8s::state::evmap::Value<Namespace>>,
        fields_spec: FieldsSpec,
    ) -> Self {
        Self {
            namespace_state_reader,
            fields_spec,
        }
    }
}

impl NamespaceMetadataAnnotator {
    /// Annotates an event with the information from the [`Namespace::metadata`].
    /// The event has to have a [`POD_NAMESPACE`] field set.
    pub fn annotate<'a>(&self, event: &mut Event, pod_namespace: &str) {
        let log = event.as_mut_log();
        let guard = self
            .namespace_state_reader
            .get(pod_namespace)
            .expect("pod_namespace not found in state");
        let entry = guard.get_one().unwrap();
        let namespace: &Namespace = entry.as_ref();

        annotate_from_metadata(log, &self.fields_spec, &namespace.metadata);
    }
}

fn annotate_from_metadata(log: &mut LogEvent, fields_spec: &FieldsSpec, metadata: &ObjectMeta) {
    // Calculate and cache the prefix path.
    let prefix_path = PathIter::new(fields_spec.namespace_labels.as_ref()).collect::<Vec<_>>();
    for (key, val) in metadata.labels.iter() {
        let mut path = prefix_path.clone();
        path.push(PathComponent::Key(key.clone()));
        log.insert_path(path, val.to_owned());
    }
}
