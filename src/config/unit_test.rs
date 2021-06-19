use super::{Config, ConfigBuilder, TestDefinition, TestInput, TestInputValue};
use crate::config::{self, ConfigPath, GlobalOptions, TransformConfig};
use crate::{
    conditions::Condition,
    event::{Event, Value},
    transforms::Transform,
};
use indexmap::IndexMap;
use std::collections::HashMap;

pub async fn build_unit_tests_main(paths: &[ConfigPath]) -> Result<Vec<UnitTest>, Vec<String>> {
    config::init_log_schema(paths, false)?;

    let (config, _) = super::loading::load_builder_from_paths(paths)?;

    build_unit_tests(config).await
}

async fn build_unit_tests(mut builder: ConfigBuilder) -> Result<Vec<UnitTest>, Vec<String>> {
    let mut tests = vec![];
    let mut errors = vec![];

    let expansions = super::compiler::expand_macros(&mut builder)?;

    // Don't let this escape since it's not validated
    let config = Config {
        global: builder.global,
        #[cfg(feature = "api")]
        api: builder.api,
        healthchecks: builder.healthchecks,
        sources: builder.sources,
        sinks: builder.sinks,
        transforms: builder.transforms,
        tests: builder.tests,
        expansions,
    };

    for test in &config.tests {
        match build_unit_test(test, &config).await {
            Ok(t) => tests.push(t),
            Err(errs) => {
                let mut test_err = errs.join("\n");
                // Indent all line breaks
                test_err = test_err.replace("\n", "\n  ");
                test_err.insert_str(0, &format!("Failed to build test '{}':\n  ", test.name));
                errors.push(test_err);
            }
        }
    }

    if errors.is_empty() {
        Ok(tests)
    } else {
        Err(errors)
    }
}

pub struct UnitTest {
    pub name: String,
    inputs: Vec<(Vec<String>, Event)>,
    transforms: IndexMap<String, UnitTestTransform>,
    checks: Vec<UnitTestCheck>,
    no_outputs_from: Vec<String>,
    globals: GlobalOptions,
}

struct UnitTestTransform {
    transform: Transform<Event>,
    config: Box<dyn TransformConfig>,
    next: Vec<String>,
}

struct UnitTestCheck {
    extract_from: String,
    conditions: Vec<Box<dyn Condition>>,
}

fn event_to_string(event: &Event) -> String {
    match event {
        Event::Log(log) => serde_json::to_string(&log).unwrap_or_else(|_| "{}".into()),
        Event::Metric(metric) => serde_json::to_string(&metric).unwrap_or_else(|_| "{}".into()),
    }
}

fn events_to_string(name: &str, events: &[Event]) -> String {
    if events.len() > 1 {
        format!(
            "  {}s:\n    {}",
            name,
            events
                .iter()
                .map(|e| event_to_string(e))
                .collect::<Vec<_>>()
                .join("\n    ")
        )
    } else {
        events
            .first()
            .map(|e| format!("  {}: {}", name, event_to_string(e)))
            .unwrap_or(format!("  no {}", name))
    }
}

fn walk(
    node: &str,
    mut inputs: Vec<Event>,
    transforms: &mut IndexMap<String, UnitTestTransform>,
    aggregated_results: &mut HashMap<String, (Vec<Event>, Vec<Event>)>,
    globals: &GlobalOptions,
) {
    let mut results = Vec::new();
    let mut targets = Vec::new();

    // Use `remove` to take ownership.
    if let Some((key, mut target)) = transforms.remove_entry(node) {
        match target.transform {
            Transform::Function(ref mut t) => {
                for input in inputs.clone() {
                    t.transform(&mut results, input)
                }
                targets = target.next.clone();
                transforms.insert(key, target);
            }
            Transform::Task(t) => {
                error!("Using a recently refactored `TaskTransform` in a unit test. You may experience limited support for multiple inputs.");
                let in_stream = futures::stream::iter(inputs.clone());
                let out_stream = t.transform(Box::pin(in_stream));
                // TODO(new-transform-enum): Handle Many
                let out_iter = futures::executor::block_on_stream(out_stream);
                results.extend(out_iter);
                targets = target.next.clone();
                // TODO: This is a hack.
                // Our tasktransforms must consume the transform to attach it to an input stream, so we rebuild it between input streams.
                transforms.insert(key, UnitTestTransform {
                    transform:  futures::executor::block_on(target.config.clone().build(globals))
                        .expect("Failed to build a known valid transform config. Things may have changed during runtime."),
                    config: target.config,
                    next: target.next
                });
            }
        }
    }

    for child in targets {
        walk(
            &child,
            results.clone(),
            transforms,
            aggregated_results,
            globals,
        );
    }

    if let Some((mut e_inputs, mut e_results)) = aggregated_results.remove(node) {
        inputs.append(&mut e_inputs);
        results.append(&mut e_results);
    }
    aggregated_results.insert(node.into(), (inputs, results));
}

impl UnitTest {
    // Executes each test and provides a tuple of inspections and error lists.
    pub fn run(&mut self) -> (Vec<String>, Vec<String>) {
        let mut errors = Vec::new();
        let mut inspections = Vec::new();
        let mut results = HashMap::new();

        let mut inputs_by_target = HashMap::new();
        for (targets, event) in &self.inputs {
            for target in targets {
                let entry = inputs_by_target
                    .entry(target.clone())
                    .or_insert_with(Vec::new);
                entry.push(event.clone());
            }
        }

        for (target, inputs) in inputs_by_target {
            walk(
                &target,
                inputs,
                &mut self.transforms,
                &mut results,
                &self.globals,
            );
        }

        for check in &self.checks {
            if let Some((inputs, outputs)) = results.get(&check.extract_from) {
                if check.conditions.is_empty() {
                    inspections.push(format!(
                        "check transform '{}' payloads (events encoded as JSON):\n{}\n{}",
                        check.extract_from,
                        events_to_string(" input", inputs),
                        events_to_string("output", outputs),
                    ));
                    continue;
                }
                let failed_conditions = check
                    .conditions
                    .iter()
                    .enumerate()
                    .flat_map(|(i, cond)| {
                        let cond_errs = outputs
                            .iter()
                            .enumerate()
                            .filter_map(|(j, e)| {
                                cond.check_with_context(e).err().map(|err| {
                                    if outputs.len() > 1 {
                                        format!("condition[{}], payload[{}]: {}", i, j, err)
                                    } else {
                                        format!("condition[{}]: {}", i, err)
                                    }
                                })
                            })
                            .collect::<Vec<_>>();
                        if cond_errs.len() < outputs.len() {
                            // At least one output succeeded for this condition.
                            Vec::new()
                        } else {
                            cond_errs
                        }
                    })
                    .collect::<Vec<_>>();
                if !failed_conditions.is_empty() {
                    errors.push(format!(
                        "check transform '{}' failed conditions:\n  {}\npayloads (events encoded as JSON):\n{}\n{}",
                        check.extract_from,
                        failed_conditions.join("\n  "),
                        events_to_string(" input", inputs),
                        events_to_string("output", outputs),
                    ));
                }
                if outputs.is_empty() {
                    errors.push(format!(
                        "check transform '{}' failed, no events received.",
                        check.extract_from,
                    ));
                }
            } else {
                errors.push(format!(
                    "check transform '{}' failed: received zero resulting events.",
                    check.extract_from,
                ));
            }
        }

        for tform in &self.no_outputs_from {
            if let Some((inputs, outputs)) = results.get(tform) {
                if !outputs.is_empty() {
                    errors.push(format!(
                        "check transform '{}' failed: expected no outputs.\npayloads (events encoded as JSON):\n{}\n{}",
                        tform,
                        events_to_string(" input", inputs),
                        events_to_string("output", outputs),
                    ));
                }
            }
        }

        (inspections, errors)
    }
}

//------------------------------------------------------------------------------

fn links_to_a_leaf(
    target: &str,
    leaves: &IndexMap<String, ()>,
    link_checked: &mut IndexMap<String, bool>,
    transform_outputs: &IndexMap<String, IndexMap<String, ()>>,
) -> bool {
    if *link_checked.get(target).unwrap_or(&false) {
        return true;
    }
    let has_linked_children = if let Some(outputs) = transform_outputs.get(target) {
        outputs
            .iter()
            .filter(|(o, _)| links_to_a_leaf(o, leaves, link_checked, transform_outputs))
            .count()
            > 0
    } else {
        false
    };
    let linked = leaves.contains_key(target) || has_linked_children;
    link_checked.insert(target.to_owned(), linked);
    linked
}

/// Reduces a collection of transforms into a set that only contains those that
/// link between our root (test input) and a set of leaves (test outputs).
fn reduce_transforms(
    roots: Vec<String>,
    leaves: &IndexMap<String, ()>,
    transform_outputs: &mut IndexMap<String, IndexMap<String, ()>>,
) {
    let mut link_checked: IndexMap<String, bool> = IndexMap::new();

    if roots
        .iter()
        .map(|r| links_to_a_leaf(r, leaves, &mut link_checked, transform_outputs))
        .collect::<Vec<_>>() // Ensure we map each element.
        .iter()
        .all(|b| !b)
    {
        transform_outputs.clear();
    }

    transform_outputs.retain(|name, children| {
        let linked = roots.contains(name) || *link_checked.get(name).unwrap_or(&false);
        if linked {
            // Also remove all unlinked children.
            children.retain(|child_name, _| {
                roots.contains(child_name) || *link_checked.get(child_name).unwrap_or(&false)
            })
        }
        linked
    });
}

fn build_input(config: &Config, input: &TestInput) -> Result<(Vec<String>, Event), String> {
    let target = config.get_inputs(&input.insert_at);

    match input.type_str.as_ref() {
        "raw" => match input.value.as_ref() {
            Some(v) => Ok((target, Event::from(v.clone()))),
            None => Err("input type 'raw' requires the field 'value'".to_string()),
        },
        "log" => {
            if let Some(log_fields) = &input.log_fields {
                let mut event = Event::from("");
                for (path, value) in log_fields {
                    let value: Value = match value {
                        TestInputValue::String(s) => Value::from(s.to_owned()),
                        TestInputValue::Boolean(b) => Value::from(*b),
                        TestInputValue::Integer(i) => Value::from(*i),
                        TestInputValue::Float(f) => Value::from(*f),
                    };
                    event.as_mut_log().insert(path.to_owned(), value);
                }
                Ok((target, event))
            } else {
                Err("input type 'log' requires the field 'log_fields'".to_string())
            }
        }
        "metric" => {
            if let Some(metric) = &input.metric {
                Ok((target, Event::Metric(metric.clone())))
            } else {
                Err("input type 'metric' requires the field 'metric'".to_string())
            }
        }
        _ => Err(format!(
            "unrecognized input type '{}', expected one of: 'raw', 'log' or 'metric'",
            input.type_str
        )),
    }
}

fn build_inputs(
    config: &Config,
    definition: &TestDefinition,
) -> Result<Vec<(Vec<String>, Event)>, Vec<String>> {
    let mut inputs = Vec::new();
    let mut errors = vec![];

    if let Some(input_def) = &definition.input {
        match build_input(config, input_def) {
            Ok(input_event) => inputs.push(input_event),
            Err(err) => errors.push(err),
        }
    } else if definition.inputs.is_empty() {
        errors.push("must specify at least one input.".to_owned());
    }
    for input_def in &definition.inputs {
        match build_input(config, input_def) {
            Ok(input_event) => inputs.push(input_event),
            Err(err) => errors.push(err),
        }
    }

    if errors.is_empty() {
        Ok(inputs)
    } else {
        Err(errors)
    }
}

async fn build_unit_test(
    definition: &TestDefinition,
    config: &Config,
) -> Result<UnitTest, Vec<String>> {
    let mut errors = vec![];

    let inputs = match build_inputs(config, definition) {
        Ok(inputs) => inputs,
        Err(mut errs) => {
            errors.append(&mut errs);
            Vec::new()
        }
    };

    // Maps transform names with their output targets (transforms that use it as
    // an input).
    let mut transform_outputs: IndexMap<String, IndexMap<String, ()>> = config
        .transforms
        .iter()
        .map(|(k, _)| (k.clone(), IndexMap::new()))
        .collect();

    config.transforms.iter().for_each(|(k, t)| {
        t.inputs.iter().for_each(|i| {
            if let Some(outputs) = transform_outputs.get_mut(i) {
                outputs.insert(k.to_string(), ());
            }
        })
    });

    for (i, (input_target, _)) in inputs.iter().enumerate() {
        for target in input_target {
            if !transform_outputs.contains_key(target) {
                errors.push(format!(
                    "inputs[{}]: unable to locate target transform '{}'",
                    i, target
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut leaves: IndexMap<String, ()> = IndexMap::new();
    definition.outputs.iter().for_each(|o| {
        leaves.insert(o.extract_from.clone(), ());
    });
    definition.no_outputs_from.iter().for_each(|o| {
        leaves.insert(o.clone(), ());
    });

    // Reduce the configured transforms into just the ones connecting our test
    // target with output targets.
    reduce_transforms(
        inputs
            .iter()
            .map(|(names, _)| names)
            .flatten()
            .cloned()
            .collect::<Vec<_>>(),
        &leaves,
        &mut transform_outputs,
    );

    // Build reduced transforms.
    let mut transforms: IndexMap<String, UnitTestTransform> = IndexMap::new();
    for (name, transform_config) in &config.transforms {
        if let Some(outputs) = transform_outputs.remove(name) {
            match transform_config.inner.build(&config.global).await {
                Ok(transform) => {
                    transforms.insert(
                        name.clone(),
                        UnitTestTransform {
                            transform,
                            config: transform_config.inner.clone(),
                            next: outputs.into_iter().map(|(k, _)| k).collect(),
                        },
                    );
                }
                Err(err) => {
                    errors.push(format!(
                        "failed to build transform '{}': {:#}",
                        name,
                        anyhow::anyhow!(err)
                    ));
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    definition.outputs.iter().for_each(|o| {
        if !transforms.contains_key(&o.extract_from) {
            let targets = inputs.iter().map(|(i, _)| i).flatten().collect::<Vec<_>>();
            if targets.len() == 1 {
                errors.push(format!(
                    "unable to complete topology between target transform '{}' and output target '{}'",
                    targets.first().unwrap(), o.extract_from
                ));
            } else {
                errors.push(format!(
                    "unable to complete topology between target transforms {:?} and output target '{}'",
                    targets, o.extract_from
                ));
            }
        }
    });

    // Build all output conditions.
    let checks = definition
        .outputs
        .iter()
        .map(|o| {
            let mut conditions: Vec<Box<dyn Condition>> = Vec::new();
            for (index, cond_conf) in o
                .conditions
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .enumerate()
            {
                match cond_conf.build() {
                    Ok(c) => conditions.push(c),
                    Err(e) => errors.push(format!(
                        "failed to create test condition '{}': {}",
                        index, e,
                    )),
                }
            }

            UnitTestCheck {
                extract_from: o.extract_from.clone(),
                conditions,
            }
        })
        .collect();

    if definition.outputs.is_empty() && definition.no_outputs_from.is_empty() {
        errors.push(
            "unit test must contain at least one of `outputs` or `no_outputs_from`.".to_owned(),
        );
    }

    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok(UnitTest {
            name: definition.name.clone(),
            inputs,
            transforms,
            checks,
            no_outputs_from: definition.no_outputs_from.clone(),
            globals: config.global.clone(),
        })
    }
}

#[cfg(all(test, feature = "transforms-add_fields", feature = "transforms-route"))]
mod tests {
    use super::*;
    use crate::config::ConfigBuilder;
    use indoc::indoc;

    #[tokio::test]
    async fn parse_no_input() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                my_string_field = "string value"

              [[tests]]
                name = "broken test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
        "#})
        .unwrap();

        let errs = build_unit_tests(config).await.err().unwrap();
        assert_eq!(
            errs,
            vec![indoc! {r#"
                Failed to build test 'broken test':
                  inputs[0]: unable to locate target transform 'foo'"#}
            .to_owned(),]
        );

        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                my_string_field = "string value"

              [[tests]]
                name = "broken test"

              [[tests.inputs]]
                insert_at = "bar"
                value = "nah this doesnt matter"

              [[tests.inputs]]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
        "#})
        .unwrap();

        let errs = build_unit_tests(config).await.err().unwrap();
        assert_eq!(
            errs,
            vec![indoc! {r#"
                Failed to build test 'broken test':
                  inputs[1]: unable to locate target transform 'foo'"#}
            .to_owned(),]
        );
    }

    #[tokio::test]
    async fn parse_no_test_input() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                my_string_field = "string value"

              [[tests]]
                name = "broken test"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
        "#})
        .unwrap();

        let errs = build_unit_tests(config).await.err().unwrap();
        assert_eq!(
            errs,
            vec![indoc! {r#"
                Failed to build test 'broken test':
                  must specify at least one input."#}
            .to_owned(),]
        );
    }

    #[tokio::test]
    async fn parse_no_outputs() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                my_string_field = "string value"

              [[tests]]
                name = "broken test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"
        "#})
        .unwrap();

        let errs = build_unit_tests(config).await.err().unwrap();
        assert_eq!(
            errs,
            vec![indoc! {r#"
                Failed to build test 'broken test':
                  unit test must contain at least one of `outputs` or `no_outputs_from`."#}
            .to_owned(),]
        );
    }

    #[tokio::test]
    async fn parse_broken_topology() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = ["something"]
              type = "add_fields"
              [transforms.foo.fields]
                foo_field = "string value"

            [transforms.nah]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.nah.fields]
                new_field = "string value"

            [transforms.baz]
              inputs = ["bar"]
              type = "add_fields"
              [transforms.baz.fields]
                baz_field = "string value"

            [transforms.quz]
              inputs = ["bar"]
              type = "add_fields"
              [transforms.quz.fields]
                quz_field = "string value"

            [[tests]]
              name = "broken test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "not this"

              [[tests.outputs]]
                extract_from = "quz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "not this"

            [[tests]]
              name = "broken test 2"

              [tests.input]
                insert_at = "nope"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "quz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "not this"

            [[tests]]
              name = "broken test 3"

              [[tests.inputs]]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.inputs]]
                insert_at = "nah"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "not this"

              [[tests.outputs]]
                extract_from = "quz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "not this"
        "#})
        .unwrap();

        let errs = build_unit_tests(config).await.err().unwrap();
        assert_eq!(
            errs,
            vec![
                r#"Failed to build test 'broken test':
  unable to complete topology between target transform 'foo' and output target 'baz'
  unable to complete topology between target transform 'foo' and output target 'quz'"#
                    .to_owned(),
                r#"Failed to build test 'broken test 2':
  inputs[0]: unable to locate target transform 'nope'"#
                    .to_owned(),
                r#"Failed to build test 'broken test 3':
  unable to complete topology between target transforms ["foo", "nah"] and output target 'baz'
  unable to complete topology between target transforms ["foo", "nah"] and output target 'quz'"#
                    .to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn parse_bad_input_event() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                my_string_field = "string value"

              [[tests]]
                name = "broken test"

              [tests.input]
                insert_at = "foo"
                type = "nah"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
        "#})
        .unwrap();

        let errs = build_unit_tests(config).await.err().unwrap();
        assert_eq!(
            errs,
            vec![indoc! {r#"
                Failed to build test 'broken test':
                  unrecognized input type 'nah', expected one of: 'raw', 'log' or 'metric'"#}
            .to_owned(),]
        );
    }

    #[tokio::test]
    async fn test_success_multi_inputs() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                new_field = "string value"

            [transforms.foo_two]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo_two.fields]
                new_field_two = "second string value"

            [transforms.bar]
              inputs = ["foo", "foo_two"]
              type = "add_fields"
              [transforms.bar.fields]
                second_new_field = "also a string value"

            [transforms.baz]
              inputs = ["bar"]
              type = "add_fields"
              [transforms.baz.fields]
                third_new_field = "also also a string value"

            [[tests]]
              name = "successful test"

              [[tests.inputs]]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.inputs]]
                insert_at = "foo_two"
                value = "nah this also doesnt matter"

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "message.equals" = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "foo_two"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field_two.equals" = "second string value"
                  "message.equals" = "nah this also doesnt matter"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also a string value"
                  "message.equals" = "nah this doesnt matter"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field_two.equals" = "second string value"
                  "second_new_field.equals" = "also a string value"
                  "message.equals" = "nah this also doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also a string value"
                  "third_new_field.equals" = "also also a string value"
                  "message.equals" = "nah this doesnt matter"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field_two.equals" = "second string value"
                  "second_new_field.equals" = "also a string value"
                  "third_new_field.equals" = "also also a string value"
                  "message.equals" = "nah this also doesnt matter"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_success() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                new_field = "string value"

            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                second_new_field = "also a string value"

            [transforms.baz]
              inputs = ["bar"]
              type = "add_fields"
              [transforms.baz.fields]
                third_new_field = "also also a string value"

            [[tests]]
              name = "successful test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "message.equals" = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also a string value"
                  "message.equals" = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also a string value"
                  "third_new_field.equals" = "also also a string value"
                  "message.equals" = "nah this doesnt matter"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_route() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "route"
              [transforms.foo.route.first]
                type = "check_fields"
                "message.eq" = "test swimlane 1"
              [transforms.foo.route.second]
                type = "check_fields"
                "message.eq" = "test swimlane 2"

              [transforms.bar]
                inputs = ["foo.first"]
                type = "add_fields"
              [transforms.bar.fields]
                new_field = "new field added"

            [[tests]]
              name = "successful route test 1"

              [tests.input]
                insert_at = "foo"
                value = "test swimlane 1"

              [[tests.outputs]]
                extract_from = "foo.first"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "test swimlane 1"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "test swimlane 1"
                  "new_field.equals" = "new field added"

            [[tests]]
              name = "successful route test 2"

              [tests.input]
                insert_at = "foo"
                value = "test swimlane 2"

              [[tests.outputs]]
                extract_from = "foo.second"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "test swimlane 2"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_fail_no_outputs() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = [ "TODO" ]
              type = "field_filter"
              field = "not_exist"
              value = "not_value"

              [[tests]]
                name = "check_no_outputs"
                [tests.input]
                  insert_at = "foo"
                  type = "raw"
                  value = "test value"

                [[tests.outputs]]
                  extract_from = "foo"
                  [[tests.outputs.conditions]]
                    type = "check_fields"
                    "message.equals" = "test value"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_ne!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_fail_two_output_events() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = [ "TODO" ]
              type = "add_fields"
              [transforms.foo.fields]
                foo = "new field 1"

            [transforms.bar]
              inputs = [ "foo" ]
              type = "add_fields"
              [transforms.bar.fields]
                bar = "new field 2"

            [transforms.baz]
              inputs = [ "foo" ]
              type = "add_fields"
              [transforms.baz.fields]
                baz = "new field 3"

            [transforms.boo]
              inputs = [ "bar", "baz" ]
              type = "add_fields"
              [transforms.boo.fields]
                boo = "new field 4"

            [[tests]]
              name = "check_multi_payloads"

              [tests.input]
                insert_at = "foo"
                type = "raw"
                value = "first"

              [[tests.outputs]]
                extract_from = "boo"

                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "baz.equals" = "new field 3"

                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "bar.equals" = "new field 2"

            [[tests]]
              name = "check_multi_payloads_bad"

              [tests.input]
                insert_at = "foo"
                type = "raw"
                value = "first"

              [[tests.outputs]]
                extract_from = "boo"

                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "baz.equals" = "new field 3"
                  "bar.equals" = "new field 2"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
        assert_ne!(tests[1].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_no_outputs_from() {
        let config: ConfigBuilder = toml::from_str(indoc! {r#"
            [transforms.foo]
              inputs = [ "ignored" ]
              type = "field_filter"
              field = "message"
              value = "foo"

            [[tests]]
              name = "check_no_outputs_from_succeeds"
              no_outputs_from = [ "foo" ]

              [tests.input]
                insert_at = "foo"
                type = "raw"
                value = "not foo at all"

            [[tests]]
              name = "check_no_outputs_from_fails"
              no_outputs_from = [ "foo" ]

              [tests.input]
                insert_at = "foo"
                type = "raw"
                value = "foo"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
        assert_ne!(tests[1].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_no_outputs_from_chained() {
        let config: ConfigBuilder = toml::from_str(indoc! { r#"
            [transforms.foo]
              inputs = [ "ignored" ]
              type = "field_filter"
              field = "message"
              value = "foo"

            [transforms.bar]
              inputs = [ "foo" ]
              type = "add_fields"
              [transforms.bar.fields]
                bar = "new field"

            [[tests]]
              name = "check_no_outputs_from_succeeds"
              no_outputs_from = [ "bar" ]

              [tests.input]
                insert_at = "foo"
                type = "raw"
                value = "not foo at all"

            [[tests]]
              name = "check_no_outputs_from_fails"
              no_outputs_from = [ "bar" ]

              [tests.input]
                insert_at = "foo"
                type = "raw"
                value = "foo"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
        assert_ne!(tests[1].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_log_input() {
        let config: ConfigBuilder = toml::from_str(indoc! { r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                new_field = "string value"

            [[tests]]
              name = "successful test with log event"

              [tests.input]
                insert_at = "foo"
                type = "log"
                [tests.input.log_fields]
                  message = "this is the message"
                  int_val = 5
                  bool_val = true

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "message.equals" = "this is the message"
                  "bool_val.eq" = true
                  "int_val.eq" = 5
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_metric_input() {
        let config: ConfigBuilder = toml::from_str(indoc! { r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_tags"
              [transforms.foo.tags]
                new_tag = "new value added"

            [[tests]]
              name = "successful test with metric event"

              [tests.input]
                insert_at = "foo"
                type = "metric"
                [tests.input.metric]
                  kind = "incremental"
                  name = "foometric"
                  [tests.input.metric.tags]
                    tagfoo = "valfoo"
                  [tests.input.metric.counter]
                    value = 100.0

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "tagfoo.equals" = "valfoo"
                  "new_tag.eq" = "new value added"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_success_over_gap() {
        let config: ConfigBuilder = toml::from_str(indoc! { r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                new_field = "string value"

            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                second_new_field = "also a string value"

            [transforms.baz]
              inputs = ["bar"]
              type = "add_fields"
              [transforms.baz.fields]
                third_new_field = "also also a string value"

            [[tests]]
              name = "successful test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also a string value"
                  "third_new_field.equals" = "also also a string value"
                  "message.equals" = "nah this doesnt matter"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_success_tree() {
        let config: ConfigBuilder = toml::from_str(indoc! { r#"
            [transforms.ignored]
              inputs = ["also_ignored"]
              type = "add_fields"
              [transforms.ignored.fields]
                not_field = "string value"

            [transforms.foo]
              inputs = ["ignored"]
              type = "add_fields"
              [transforms.foo.fields]
                new_field = "string value"

            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                second_new_field = "also a string value"

            [transforms.baz]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.baz.fields]
                second_new_field = "also also a string value"

            [[tests]]
              name = "successful test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also a string value"
                  "message.equals" = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "new_field.equals" = "string value"
                  "second_new_field.equals" = "also also a string value"
                  "message.equals" = "nah this doesnt matter"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_eq!(tests[0].run().1, Vec::<String>::new());
    }

    #[tokio::test]
    async fn test_fails() {
        let config: ConfigBuilder = toml::from_str(indoc! { r#"
            [transforms.foo]
              inputs = ["ignored"]
              type = "remove_fields"
              fields = ["timestamp"]

            [transforms.bar]
              inputs = ["foo"]
              type = "add_fields"
              [transforms.bar.fields]
                second_new_field = "also a string value"

            [transforms.baz]
              inputs = ["bar"]
              type = "add_fields"
              [transforms.baz.fields]
                third_new_field = "also also a string value"

            [[tests]]
              name = "failing test"

              [tests.input]
                insert_at = "foo"
                value = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "nah this doesnt matter"

              [[tests.outputs]]
                extract_from = "bar"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "not this"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "second_new_field.equals" = "and not this"

            [[tests]]
              name = "another failing test"

              [tests.input]
                insert_at = "foo"
                value = "also this doesnt matter"

              [[tests.outputs]]
                extract_from = "foo"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "message.equals" = "also this doesnt matter"

              [[tests.outputs]]
                extract_from = "baz"
                [[tests.outputs.conditions]]
                  type = "check_fields"
                  "second_new_field.equals" = "nope not this"
                  "third_new_field.equals" = "and not this"
                  "message.equals" = "also this doesnt matter"
        "#})
        .unwrap();

        let mut tests = build_unit_tests(config).await.unwrap();
        assert_ne!(tests[0].run().1, Vec::<String>::new());
        assert_ne!(tests[1].run().1, Vec::<String>::new());
        // TODO: The json representations are randomly ordered so these checks
        // don't always pass:
        /*
                assert_eq!(
                    tests[0].run().1,
                    vec![r#"check transform 'bar' failed conditions:
          condition[0]: predicates failed: [ message.equals: 'not this' ]
          condition[1]: predicates failed: [ second_new_field.equals: 'and not this' ]
        payloads (JSON encoded):
          input: {"message":"nah this doesnt matter"}
          output: {"message":"nah this doesnt matter","second_new_field":"also a string value"}"#.to_owned(),
                    ]);
                assert_eq!(
                    tests[1].run().1,
                    vec![r#"check transform 'baz' failed conditions:
          condition[0]: predicates failed: [ second_new_field.equals: 'nope not this', third_new_field.equals: 'and not this' ]
        payloads (JSON encoded):
          input: {"second_new_field":"also a string value","message":"also this doesnt matter"}
          output: {"third_new_field":"also also a string value","second_new_field":"also a string value","message":"also this doesnt matter"}"#.to_owned(),
                    ]);
                */
    }
}
