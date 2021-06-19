use crate::event::Event;
use futures::{stream, Stream, StreamExt};
use std::pin::Pin;

#[cfg(any(feature = "lua"))]
pub mod runtime_transform;

/// Transforms come in two variants. Functions, or tasks.
///
/// While function transforms can be run out of order, or concurrently, task
/// transforms act as a coordination or barrier point.
pub enum Transform<T: Clone + Send + 'static> {
    Function(Box<dyn FunctionTransform<T>>),
    Task(Box<dyn TaskTransform<T>>),
}

impl<T: Clone + Send> Transform<T> {
    /// Create a new function transform.
    ///
    /// These functions are "stateless" and can be run in parallel, without
    /// regard for coordination.
    ///
    /// **Note:** You should prefer to implement this over [`TaskTransform`]
    /// where possible.
    pub fn function(v: impl FunctionTransform<T> + 'static) -> Self {
        Transform::Function(Box::new(v))
    }

    /// Mutably borrow the inner transform as a function transform.
    ///
    /// # Panics
    ///
    /// If the transform is a [`TaskTransform`] this will panic.
    pub fn as_function(&mut self) -> &mut Box<dyn FunctionTransform<T>> {
        match self {
            Transform::Function(t) => t,
            Transform::Task(_) => panic!(
                "Called `Transform::as_function` on something that was not a function variant."
            ),
        }
    }

    /// Transmute the inner transform into a function transform.
    ///
    /// # Panics
    ///
    /// If the transform is a [`TaskTransform`] this will panic.
    pub fn into_function(self) -> Box<dyn FunctionTransform<T>> {
        match self {
            Transform::Function(t) => t,
            Transform::Task(_) => panic!(
                "Called `Transform::into_function` on something that was not a function variant."
            ),
        }
    }

    /// Create a new task transform.
    ///
    /// These tasks are coordinated, and map a stream of some `U` to some other
    /// `T`.
    ///
    /// **Note:** You should prefer to implement [`FunctionTransform`] over this
    /// where possible.
    pub fn task(v: impl TaskTransform<T> + 'static) -> Self {
        Transform::Task(Box::new(v))
    }

    /// Mutably borrow the inner transform as a task transform.
    ///
    /// # Panics
    ///
    /// If the transform is a [`FunctionTransform`] this will panic.
    pub fn as_task(&mut self) -> &mut Box<dyn TaskTransform<T>> {
        match self {
            Transform::Function(_) => {
                panic!("Called `Transform::as_task` on something that was not a task variant.")
            }
            Transform::Task(t) => t,
        }
    }

    /// Transmute the inner transform into a task transform.
    ///
    /// # Panics
    ///
    /// If the transform is a [`FunctionTransform`] this will panic.
    pub fn into_task(self) -> Box<dyn TaskTransform<T>> {
        match self {
            Transform::Function(_) => {
                panic!("Called `Transform::into_task` on something that was not a task variant.")
            }
            Transform::Task(t) => t,
        }
    }

    /// Apply the transform to an input stream.
    pub fn transform(
        self,
        input: impl Stream<Item = T> + Send + 'static,
    ) -> impl Stream<Item = T> + Send {
        match self {
            Transform::Function(mut function) => input
                .flat_map(move |event| {
                    let mut buf = Vec::with_capacity(1);
                    function.transform(&mut buf, event);
                    stream::iter(buf.into_iter())
                })
                .boxed(),
            Transform::Task(task) => task.transform(input.boxed()),
        }
    }
}

/// Transforms that are simple, and don't require attention to coordination.
/// You can run them as simple functions over events in any order.
///
/// # Invariants
///
/// * It is an illegal invariant to implement `FunctionTransform` for a
///   `TaskTransform` or vice versa.
pub trait FunctionTransform<T: Clone + Send>: Send + Sync + dyn_clone::DynClone {
    fn transform(&mut self, output: &mut Vec<T>, input: T);
}

dyn_clone::clone_trait_object!(FunctionTransform<Event>);

/// Transforms that tend to be more complicated runtime style components.
///
/// These require coordination and map a stream of some `T` to some `U`.
///
/// # Invariants
///
/// * It is an illegal invariant to implement `FunctionTransform` for a
/// `TaskTransform` or vice versa.
pub trait TaskTransform<T: Send>: Send {
    fn transform(
        self: Box<Self>,
        task: Pin<Box<dyn Stream<Item = T> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = T> + Send>>
    where
        Self: 'static;
}
