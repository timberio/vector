use criterion::criterion_main;
use criterion::{criterion_group, BatchSize, BenchmarkId, Criterion};
use futures::{stream, SinkExt, Stream, StreamExt};
use indoc::indoc;
use serde_json::Value;
use std::{collections::HashMap, fs, io::Read, path::Path, pin::Pin};
use vector::{
    event::Event,
    transforms::{wasm::Wasm, TaskTransform, Transform},
};

fn parse_event_artifact(path: impl AsRef<Path>) -> vector::Result<Event> {
    let mut event = Event::new_empty_log();
    let mut test_file = fs::File::open(path)?;

    let mut buf = String::new();
    test_file.read_to_string(&mut buf)?;
    let test_json: HashMap<String, Value> = serde_json::from_str(&buf)?;

    for (key, value) in test_json {
        event.as_mut_log().insert(key, value.clone());
    }
    Ok(event)
}

pub fn protobuf(c: &mut Criterion) {
    let input = parse_event_artifact("tests/data/wasm/protobuf/demo.json").unwrap();

    c.bench_function("wasm/protobuf", |b| {
        let transform = Box::new(
            Wasm::new(
                toml::from_str(indoc! {r#"
                    module = "tests/data/wasm/protobuf/target/wasm32-wasi/release/protobuf.wasm"
                    artifact_cache = "target/artifacts/"
                "#})
                .unwrap(),
            )
            .unwrap(),
        );

        let (tx, rx) = futures::channel::mpsc::channel::<Event>(1);
        let mut rx = transform.transform(Box::pin(rx));

        b.iter_batched(
            || (tx.clone(), input.clone()),
            |(mut tx, input)| {
                futures::executor::block_on(tx.send(input)).unwrap();
                futures::executor::block_on(rx.next())
            },
            BatchSize::SmallInput,
        )
    });
}

pub fn add_fields(criterion: &mut Criterion) {
    let transforms: Vec<(&str, Transform<Event>)> = vec![
        (
            "lua",
            Transform::task(
                vector::transforms::lua::v2::Lua::new(
                    &toml::from_str(
                        indoc! {r#"
                            hooks.process = """
                            function (event, emit)
                              event.log.test_key = "test_value"
                              event.log.test_key2 = "test_value2"
                              emit(event)
                            end
                            """
                        "#},
                    )
                    .unwrap(),
                )
                .unwrap(),
            ),
        ),
        (
            "remap",
            Transform::function(
                vector::transforms::remap::Remap::new(vector::transforms::remap::RemapConfig {
                    source: indoc! {r#"
                        .test_key = "test_value"
                        .test_key2 = "test_value2"
                    "#}
                    .to_string(),
                    drop_on_error: true,
                    drop_on_abort: true,
                })
                .unwrap(),
            ),
        ),
        (
            "wasm",
            Transform::task(
                Wasm::new(
                    toml::from_str(
                        indoc! {r#"
                            module = "tests/data/wasm/add_fields/target/wasm32-wasi/release/add_fields.wasm"
                            artifact_cache = "target/artifacts/"
                        "#},
                    )
                    .unwrap(),
                )
                .unwrap(),
            ),
        ),
        (
            "native",
            Transform::function({
                let mut fields = indexmap::IndexMap::default();
                fields.insert("test_key".into(), String::from("test_value").into());
                fields.insert("test_key2".into(), String::from("test_value2").into());
                vector::transforms::add_fields::AddFields::new(fields, false).unwrap()
            }),
        ),
    ];
    let parameters = vec![0, 2, 8, 16];

    bench_group_transforms_over_parameterized_event_sizes(
        criterion,
        "wasm/add_fields",
        transforms,
        parameters,
    );
}

fn bench_group_transforms_over_parameterized_event_sizes(
    criterion: &mut Criterion,
    group: &str,
    transforms: Vec<(&str, Transform<Event>)>,
    parameters: Vec<usize>,
) {
    vector::test_util::trace_init();

    let mut group = criterion.benchmark_group(group);

    for (name, transform) in transforms {
        let (tx, rx) = futures::channel::mpsc::channel::<Event>(1);

        let mut rx: Pin<Box<dyn Stream<Item = Event> + Send>> = match transform {
            Transform::Function(t) => {
                let mut t = t.clone();
                Box::pin(rx.flat_map(move |v| {
                    let mut buf = Vec::with_capacity(1);
                    t.transform(&mut buf, v);
                    stream::iter(buf.into_iter())
                }))
            }
            Transform::Task(t) => t.transform(Box::pin(rx)),
        };

        for &parameter in &parameters {
            let mut input = Event::new_empty_log();
            for key in 0..parameter {
                input
                    .as_mut_log()
                    .insert(format!("key-{}", key), format!("value-{}", key));
            }

            let id = BenchmarkId::new(name.clone(), parameter);

            group.bench_with_input(id, &input, |bencher, input| {
                bencher.iter_batched(
                    || (tx.clone(), input.clone()),
                    |(mut tx, input)| {
                        futures::executor::block_on(tx.send(input)).unwrap();
                        futures::executor::block_on(rx.next())
                    },
                    BatchSize::SmallInput,
                )
            });
        }
    }
    group.finish();
}

criterion_group!(
    name = benches;
    // We've seen CI noise commonly be 5% so configure here
    // https://github.com/timberio/vector/issues/5394
    config = Criterion::default().noise_threshold(0.05);
    targets = protobuf, add_fields
);
criterion_main! {
    benches,
}
