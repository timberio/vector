use criterion::{criterion_group, BatchSize, Criterion, Throughput};
use futures::{stream, SinkExt, Stream, StreamExt};
use indexmap::IndexMap;
use indoc::indoc;
use std::pin::Pin;
use transforms::lua::v2::LuaConfig;
use vector::{
    config::{GlobalOptions, TransformConfig},
    event::Event,
    test_util::{collect_ready, runtime},
    transforms::{self, Transform},
};

fn bench_add_fields(c: &mut Criterion) {
    let event = Event::new_empty_log();

    let key = "the key";
    let value = "this is the value";

    let mut group = c.benchmark_group("lua/add_fields");
    group.throughput(Throughput::Elements(1));

    let benchmarks: Vec<(&str, Transform<Event>)> = vec![
        ("native", {
            let mut map = IndexMap::new();
            map.insert(String::from(key), value.to_owned().into());
            Transform::function(transforms::add_fields::AddFields::new(map, true).unwrap())
        }),
        ("v1", {
            let source = format!("event['{}'] = '{}'", key, value);

            Transform::task(transforms::lua::v1::Lua::new(source, vec![]).unwrap())
        }),
        ("v2", {
            let config = format!(
                indoc! {r#"
                    hooks.process = """
                    function (event, emit)
                      event.log['{}'] = '{}'

                      emit(event)
                    end
                    """
                "#},
                key, value
            );
            Transform::task(
                transforms::lua::v2::Lua::new(&toml::from_str::<LuaConfig>(&config).unwrap())
                    .unwrap(),
            )
        }),
    ];

    for (name, transform) in benchmarks {
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

        group.bench_function(name.to_owned(), |b| {
            b.iter_batched(
                || (tx.clone(), event.clone()),
                |(mut tx, event)| {
                    futures::executor::block_on(tx.send(event)).unwrap();
                    let transformed = futures::executor::block_on(rx.next()).unwrap();

                    debug_assert_eq!(transformed.as_log()[key], value.to_owned().into());

                    transformed
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

fn bench_field_filter(c: &mut Criterion) {
    let num_events = 10;
    let events = (0..num_events)
        .map(|i| {
            let mut event = Event::new_empty_log();
            event.as_mut_log().insert("the_field", (i % 10).to_string());
            event
        })
        .collect::<Vec<_>>();

    let mut group = c.benchmark_group("lua/field_filter");
    group.throughput(Throughput::Elements(num_events));

    let benchmarks: Vec<(&str, Transform<Event>)> = vec![
        ("native", {
            let rt = runtime();
            rt.block_on(async move {
                transforms::field_filter::FieldFilterConfig {
                    field: "the_field".to_string(),
                    value: "0".to_string(),
                }
                .build(&GlobalOptions::default())
                .await
                .unwrap()
            })
        }),
        ("v1", {
            let source = String::from(indoc! {r#"
                if event["the_field"] ~= "0" then
                    event = nil
                end
            "#});
            Transform::task(transforms::lua::v1::Lua::new(source, vec![]).unwrap())
        }),
        ("v2", {
            let config = indoc! {r#"
                hooks.process = """
                function (event, emit)
                  if event.log["the_field"] ~= "0" then
                    event = nil
                  end
                  emit(event)
                end
                """
            "#};
            Transform::task(
                transforms::lua::v2::Lua::new(&toml::from_str(config).unwrap()).unwrap(),
            )
        }),
    ];

    for (name, transform) in benchmarks {
        let (tx, rx) = futures::channel::mpsc::channel::<Event>(num_events as usize);

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

        group.bench_function(name.to_owned(), |b| {
            b.iter_batched(
                || (tx.clone(), events.clone()),
                |(mut tx, events)| {
                    let _ =
                        futures::executor::block_on(tx.send_all(&mut stream::iter(events).map(Ok)))
                            .unwrap();

                    let output = futures::executor::block_on(collect_ready(&mut rx));

                    let num = output.len();

                    debug_assert_eq!(num as u64, num_events / 10);

                    num
                },
                BatchSize::SmallInput,
            )
        });
    }

    group.finish();
}

criterion_group!(
    name = benches;
    // encapsulates CI noise we saw in
    // https://github.com/timberio/vector/issues/5394
    config = Criterion::default().noise_threshold(0.05);
    targets = bench_add_fields, bench_field_filter
);
