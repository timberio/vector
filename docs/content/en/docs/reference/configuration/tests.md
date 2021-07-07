---
title: Unit testing Vector configurations
short: Unit tests
weight: 5
aliases: ["/docs/reference/tests"]
---

Vector enables you to unit test [transforms] in your processing topology. The goal of unit tests in
general is to improve the maintainability of configurations containing larger and more complex
combinations of transforms.

Unit tests in Vector work just like unit tests for standard software libraries:

1. Provide a set of **inputs** to a transform (or a network of transforms)
1. Provide expected **outputs** from the changes made by the transform(s)
1. Receive directly actionable feedback on test failures

If *any* of your unit tests fail, the Vector instance doesn't start up. This makes unit tests a
useful guardrail for running in Vector in production settings where you need to ensure that your
topology doesn't exhibit unexpected behavior.

## Running unit tests

You can execute tests within a configuration file using the `test` subcommand:

```bash
vector test /etc/vector/vector.toml
```

You can also specify multiple files:

```bash
vector test /etc/vector/*.toml
```

{{< success >}}
Specifying multiple files would be useful here if you wanted to keep your unit tests in a separate
file from your topology configuration.
{{< /success >}}

## Configuring unit tests

{{< warning >}}
Unit tests are a bit tricky to configure. This section provides an exhaustive listing of available
configuration parameters, but if you have trouble seeing how things fit together in this section,
skip down to the [Example configuration](#examples) section for an end-to-end example that may help
illuminate the broader picture.
{{< /warning >}}

Unit tests in Vector live alongside your topology configuration. You can specify your tests in the
same config file or split them out into a separate file if you wish. The table below lists important
parameters:

Parameter | Description
:---------|:-----------
`inputs` | A table that defines a unit test [input event](#inputs).
`outputs` | A table that defines a unit test [expected output](#outputs).
`no_outputs_from` | A list of transforms that must *not* output events in order for the test to pass

### Inputs

Parameter | Description
:---------|:-----------
`insert_at` | The name of the transform into which the test input is inserted.
`log_fields` | If the transform handles [log events](#logs), these are the key/value pairs that comprise the input event.
`metric` | If the transform handles [metric events](#metrics), these are the fields

### Event types

#### Logs

#### Metrics

#### Raw events

### Outputs

## Example configuration {#examples}

Configuring unit tests is a bit tricky, so let's start with an end-to-end example. The configuration
below specifies a [`remap`][remap] transform that adds a value of `"new value"` to the
`new_field` key for each event that passes through the transform.

{{< tabs default="vector.toml" >}}
{{< tab title="vector.toml" >}}
```toml
# Configure the transform
[transforms.add_a_value]
type = "remap"
source = '''
  .new_field = "new value"
'''

# Configure the tests
[[tests]]
name = "unit tests for the remap transform"

[[tests.inputs]]
insert_at = "add_a_value"
type = "log"

[tests.inputs.log_fields]
old_field = "old value"

[[tests.outputs]]
extract_from = "add_a_value"

[[tests.outputs.conditions]]
type = "vrl"
source = '''
  .old_field == "old value" &&
  .new_field == "new value"
'''
```
{{< /tab >}}
{{< /tabs >}}

The `tests` array

{{< success title="Multiple config formats available" >}}
The example above is in TOML but Vector also supports YAML and JSON as configuration file formats.
{{< /success >}}

## Other examples

Vector's internal testing for its provided transforms includes a wide range of unit tests. We
recommend the [Vector repo][vector_tests] as a potential source of inspiration when writing your own
Vector unit tests.

[remap]: /docs/reference/configuration/transforms/remap
[transforms]: /docs/reference/glossary/#transform
[vector_tests]: https://github.com/timberio/vector/tree/master/tests/behavior/transforms
