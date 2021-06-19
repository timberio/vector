use super::{builder::ConfigBuilder, validation, Config, TransformOuter};
use indexmap::IndexMap;

pub fn compile(mut builder: ConfigBuilder) -> Result<(Config, Vec<String>), Vec<String>> {
    let mut errors = Vec::new();

    let expansions = expand_macros(&mut builder)?;

    expand_globs(&mut builder);

    let warnings = validation::warnings(&builder);

    if let Err(type_errors) = validation::check_shape(&builder) {
        errors.extend(type_errors);
    }

    if let Err(type_errors) = validation::typecheck(&builder) {
        errors.extend(type_errors);
    }

    if let Err(type_errors) = validation::check_resources(&builder) {
        errors.extend(type_errors);
    }

    if errors.is_empty() {
        Ok((
            Config {
                global: builder.global,
                #[cfg(feature = "api")]
                api: builder.api,
                healthchecks: builder.healthchecks,
                sources: builder.sources,
                sinks: builder.sinks,
                transforms: builder.transforms,
                tests: builder.tests,
                expansions,
            },
            warnings,
        ))
    } else {
        Err(errors)
    }
}

/// Some component configs can act like macros and expand themselves into multiple replacement
/// configs. Performs those expansions and records the relevant metadata.
pub(super) fn expand_macros(
    config: &mut ConfigBuilder,
) -> Result<IndexMap<String, Vec<String>>, Vec<String>> {
    let mut expanded_transforms = IndexMap::new();
    let mut expansions = IndexMap::new();
    let mut errors = Vec::new();

    while let Some((k, mut t)) = config.transforms.pop() {
        if let Some(expanded) = match t.inner.expand() {
            Ok(e) => e,
            Err(err) => {
                errors.push(format!("failed to expand transform '{}': {}", k, err));
                continue;
            }
        } {
            let mut children = Vec::new();
            for (name, child) in expanded {
                let full_name = format!("{}.{}", k, name);
                expanded_transforms.insert(
                    full_name.clone(),
                    TransformOuter {
                        inputs: t.inputs.clone(),
                        inner: child,
                    },
                );
                children.push(full_name);
            }
            expansions.insert(k.clone(), children);
        } else {
            expanded_transforms.insert(k, t);
        }
    }
    config.transforms = expanded_transforms;

    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(expansions)
    }
}

/// Expand globs in input lists
fn expand_globs(config: &mut ConfigBuilder) {
    let candidates = config
        .sources
        .keys()
        .chain(config.transforms.keys())
        .cloned()
        .collect::<Vec<String>>();

    for (name, transform) in config.transforms.iter_mut() {
        expand_globs_inner(&mut transform.inputs, name, &candidates);
    }

    for (name, sink) in config.sinks.iter_mut() {
        expand_globs_inner(&mut sink.inputs, name, &candidates);
    }
}

enum InputMatcher {
    Pattern(glob::Pattern),
    String(String),
}

impl InputMatcher {
    fn matches(&self, candidate: &str) -> bool {
        use InputMatcher::*;

        match self {
            Pattern(pattern) => pattern.matches(candidate),
            String(s) => s == candidate,
        }
    }
}

fn expand_globs_inner(inputs: &mut Vec<String>, name: &str, candidates: &[String]) {
    let raw_inputs = std::mem::take(inputs);
    for raw_input in raw_inputs {
        let matcher = glob::Pattern::new(&raw_input)
            .map(InputMatcher::Pattern)
            .unwrap_or_else(|error| {
                warn!(message = "Invalid glob pattern for input.", component_name = name, %error);
                InputMatcher::String(raw_input)
            });
        for input in candidates {
            if matcher.matches(input) && input != name {
                inputs.push(input.clone())
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{
        config::{
            DataType, GlobalOptions, SinkConfig, SinkContext, SourceConfig, SourceContext,
            TransformConfig,
        },
        sinks::{Healthcheck, VectorSink},
        sources::Source,
        transforms::Transform,
    };
    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};
    use vector_core::event::Event;

    #[derive(Debug, Serialize, Deserialize)]
    struct MockSourceConfig;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct MockTransformConfig;

    #[derive(Debug, Serialize, Deserialize)]
    struct MockSinkConfig;

    #[async_trait]
    #[typetag::serde(name = "mock")]
    impl SourceConfig for MockSourceConfig {
        async fn build(&self, _cx: SourceContext) -> crate::Result<Source> {
            unimplemented!()
        }

        fn source_type(&self) -> &'static str {
            "mock"
        }

        fn output_type(&self) -> DataType {
            DataType::Any
        }
    }

    #[async_trait]
    #[typetag::serde(name = "mock")]
    impl TransformConfig for MockTransformConfig {
        async fn build(&self, _globals: &GlobalOptions) -> crate::Result<Transform<Event>> {
            unimplemented!()
        }

        fn transform_type(&self) -> &'static str {
            "mock"
        }

        fn input_type(&self) -> DataType {
            DataType::Any
        }

        fn output_type(&self) -> DataType {
            DataType::Any
        }
    }

    #[async_trait]
    #[typetag::serde(name = "mock")]
    impl SinkConfig for MockSinkConfig {
        async fn build(&self, _cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
            unimplemented!()
        }

        fn sink_type(&self) -> &'static str {
            "mock"
        }

        fn input_type(&self) -> DataType {
            DataType::Any
        }
    }

    #[test]
    fn glob_expansion() {
        let mut builder = ConfigBuilder::default();
        builder.add_source("foo1", MockSourceConfig, None, None);
        builder.add_source("foo2", MockSourceConfig, None, None);
        builder.add_source("bar", MockSourceConfig, None, None);
        builder.add_transform("foos", &["foo*"], MockTransformConfig);
        builder.add_sink("baz", &["foos*", "b*"], MockSinkConfig);
        builder.add_sink("quix", &["*oo*"], MockSinkConfig);
        builder.add_sink("quux", &["*"], MockSinkConfig);

        let config = builder.build().expect("build should succeed");

        assert_eq!(config.transforms["foos"].inputs, vec!["foo1", "foo2"]);
        assert_eq!(config.sinks["baz"].inputs, vec!["foos", "bar"]);
        assert_eq!(
            config.sinks["quux"].inputs,
            vec!["foo1", "foo2", "bar", "foos"]
        );
        assert_eq!(config.sinks["quix"].inputs, vec!["foo1", "foo2", "foos"]);
    }
}
