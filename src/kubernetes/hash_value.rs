//! A wrapper to implement hash for k8s resource objects.

use k8s_openapi::{apimachinery::pkg::apis::meta::v1::ObjectMeta, Metadata};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use super::pod_manager_logic::extract_static_pod_config_hashsum;

/// A wrapper that provides a [`Hash`] implementation for any k8s resource
/// object.
/// Delegates to object uid for hashing and equality.
#[derive(Debug)]
pub struct HashValue<T: Metadata<Ty = ObjectMeta>>(T);

/// Used to determine what [`Metadata`] value should be used as the key
/// in [`evmap`].
#[derive(Clone, Copy)]
pub enum HashKey {
    /// metadata.uid
    Uid,
    /// meatadata.name
    Name,
}

impl<T> HashValue<T>
where
    T: Metadata<Ty = ObjectMeta>,
{
    /// Create a new [`HashValue`] by wrapping a value of `T`.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Get the `uid` from the `T`'s [`Metadata`] (if any).
    ///
    /// If the static pod config hashsum annotation exists in the metadata, it
    /// will be used instead of the mirror pod uid.
    pub fn uid(&self) -> Option<&str> {
        let metadata = self.0.metadata();
        // If static pod config hashsum annotation exists in the metadata -
        // use it instead of the uid.
        if let Some(config_hashsum) = extract_static_pod_config_hashsum(metadata) {
            return Some(config_hashsum);
        }
        Some(metadata.uid.as_ref()?.as_str())
    }

    /// Get the `namespace` from the `T`'s [`Metadata`] (if any).
    pub fn name(&self) -> Option<&str> {
        let metadata = self.0.metadata();
        Some(metadata.name.as_ref()?.as_str())
    }
}

impl<T> PartialEq<Self> for HashValue<T>
where
    T: Metadata<Ty = ObjectMeta>,
{
    fn eq(&self, other: &Self) -> bool {
        match (self.uid(), other.uid()) {
            (Some(a), Some(b)) => a.eq(b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T> Eq for HashValue<T> where T: Metadata<Ty = ObjectMeta> {}

impl<T> Hash for HashValue<T>
where
    T: Metadata<Ty = ObjectMeta>,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uid().hash(state)
    }
}

impl<T> Deref for HashValue<T>
where
    T: Metadata<Ty = ObjectMeta>,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> AsRef<T> for HashValue<T>
where
    T: Metadata<Ty = ObjectMeta>,
{
    fn as_ref(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use k8s_openapi::api::core::v1::Pod;

    use super::*;

    #[test]
    fn test_uid() {
        let cases = vec![
            // No uid or config hashsum.
            (Pod::default(), None),
            // Has uid, doesn't have a config hashsum.
            (
                Pod {
                    metadata: ObjectMeta {
                        uid: Some("uid".to_owned()),
                        ..ObjectMeta::default()
                    },
                    ..Pod::default()
                },
                Some("uid"),
            ),
            // Has both the uid and the config hashsum.
            (
                Pod {
                    metadata: ObjectMeta {
                        uid: Some("uid".to_owned()),
                        annotations: vec![(
                            "kubernetes.io/config.mirror".to_owned(),
                            "config-hashsum".to_owned(),
                        )]
                        .into_iter()
                        .collect(),
                        ..ObjectMeta::default()
                    },
                    ..Pod::default()
                },
                Some("config-hashsum"),
            ),
            // Has only the config hashsum.
            (
                Pod {
                    metadata: ObjectMeta {
                        annotations: vec![(
                            "kubernetes.io/config.mirror".to_owned(),
                            "config-hashsum".to_owned(),
                        )]
                        .into_iter()
                        .collect(),
                        ..ObjectMeta::default()
                    },
                    ..Pod::default()
                },
                Some("config-hashsum"),
            ),
        ];

        for (pod, expected) in cases {
            let hash_value = HashValue::new(pod);
            assert_eq!(hash_value.uid(), expected);
        }
    }
}
