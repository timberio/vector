use ansi_term::Colour;
use chrono::{DateTime, SecondsFormat, Utc};
use glob::glob;
use std::str::FromStr;
use structopt::StructOpt;
use vrl::{diagnostic::Formatter, state, Runtime, Terminate, Value};

use vrl_tests::{docs, Test};

#[derive(Debug, StructOpt)]
#[structopt(name = "VRL Tests", about = "Vector Remap Language Tests")]
pub struct Cmd {
    #[structopt(short, long)]
    pattern: Option<String>,

    #[structopt(short, long)]
    fail_early: bool,

    #[structopt(short, long)]
    verbose: bool,

    #[structopt(short, long)]
    no_diff: bool,

    #[structopt(long)]
    skip_functions: bool,

    /// When enabled, any log output at the INFO or above level is printed
    /// during the test run.
    #[structopt(short, long)]
    logging: bool,
}

fn should_run(name: &str, pat: &Option<String>) -> bool {
    if name == "tests/example.vrl" {
        return false;
    }

    if let Some(pat) = pat {
        if !name.contains(pat) {
            return false;
        }
    }

    true
}

fn main() {
    let cmd = Cmd::from_args();

    if cmd.logging {
        tracing_subscriber::fmt::init();
    }

    let mut failed_count = 0;
    let mut category = "".to_owned();

    let tests = glob("tests/**/*.vrl")
        .expect("valid pattern")
        .into_iter()
        .filter_map(|entry| {
            let path = entry.ok()?;
            Some(Test::from_path(&path))
        })
        .chain({
            let mut tests = vec![];
            stdlib::all().into_iter().for_each(|function| {
                function.examples().iter().for_each(|example| {
                    let test = Test::from_example(function.identifier(), example);

                    if let Some(pat) = &cmd.pattern {
                        if !format!("{}/{}", test.category, test.name).contains(pat) {
                            return;
                        }
                    }

                    tests.push(test)
                })
            });

            tests.into_iter()
        })
        .chain(docs::tests().into_iter())
        .filter(|test| should_run(&format!("{}/{}", test.category, test.name), &cmd.pattern))
        .collect::<Vec<_>>();

    for mut test in tests {
        if category != test.category {
            category = test.category.clone();
            println!("{}", Colour::Fixed(3).bold().paint(category.to_string()));
        }

        if let Some(err) = test.error {
            println!("{}", Colour::Purple.bold().paint("INVALID"));
            println!("{}", Colour::Red.paint(err));
            failed_count += 1;
            continue;
        }

        let dots = 60 - test.name.len();
        print!(
            "  {}{}",
            test.name,
            Colour::Fixed(240).paint(".".repeat(dots))
        );

        if test.skip {
            println!("{}", Colour::Yellow.bold().paint("SKIPPED"));
        }

        let state = state::Runtime::default();
        let mut runtime = Runtime::new(state);
        let program = vrl::compile(&test.source, &stdlib::all());

        let want = test.result.clone();

        match program {
            Ok(program) => {
                let result = runtime.resolve(&mut test.object, &program);

                match result {
                    Ok(got) => {
                        let got = vrl_value_to_json_value(got);
                        let mut failed = false;

                        if !test.skip {
                            let want = if want.starts_with("r'") && want.ends_with('\'') {
                                match regex::Regex::new(
                                    &want[2..want.len() - 1].replace("\\'", "'"),
                                ) {
                                    Ok(want) => want.to_string().into(),
                                    Err(_) => want.into(),
                                }
                            } else if want.starts_with("t'") && want.ends_with('\'') {
                                match DateTime::<Utc>::from_str(&want[2..want.len() - 1]) {
                                    Ok(want) => {
                                        want.to_rfc3339_opts(SecondsFormat::AutoSi, true).into()
                                    }
                                    Err(_) => want.into(),
                                }
                            } else if want.starts_with("s'") && want.ends_with('\'') {
                                want[2..want.len() - 1].into()
                            } else {
                                match serde_json::from_str::<'_, serde_json::Value>(&want.trim()) {
                                    Ok(want) => want,
                                    Err(err) => {
                                        eprintln!("{}", err);
                                        want.into()
                                    }
                                }
                            };

                            if got == want {
                                println!("{}", Colour::Green.bold().paint("OK"));
                            } else {
                                println!("{} (expectation)", Colour::Red.bold().paint("FAILED"));
                                failed_count += 1;

                                if !cmd.no_diff {
                                    let want = serde_json::to_string_pretty(&want).unwrap();
                                    let got = serde_json::to_string_pretty(&got).unwrap();

                                    let diff = prettydiff::diff_lines(&want, &got);
                                    println!("  {}", diff);
                                }

                                failed = true;
                            }
                        }

                        if cmd.verbose {
                            println!("{:#}", got);
                        }

                        if failed && cmd.fail_early {
                            std::process::exit(1)
                        }
                    }
                    Err(err) => {
                        let mut failed = false;
                        if !test.skip {
                            let got = err.to_string().trim().to_owned();
                            let want = want.trim().to_owned();

                            if (test.result_approx && compare_partial_diagnostic(&got, &want))
                                || got == want
                            {
                                println!("{}", Colour::Green.bold().paint("OK"));
                            } else if matches!(err, Terminate::Abort { .. }) {
                                let want =
                                    match serde_json::from_str::<'_, serde_json::Value>(&want) {
                                        Ok(want) => want,
                                        Err(err) => {
                                            eprintln!("{}", err);
                                            want.into()
                                        }
                                    };

                                let got = vrl_value_to_json_value(test.object.clone());
                                if got == want {
                                    println!("{} (abort)", Colour::Green.bold().paint("OK"));
                                } else {
                                    println!("{} (abort)", Colour::Red.bold().paint("FAILED"));
                                    failed_count += 1;

                                    if !cmd.no_diff {
                                        let want = serde_json::to_string_pretty(&want).unwrap();
                                        let got = serde_json::to_string_pretty(&got).unwrap();
                                        let diff = prettydiff::diff_lines(&want, &got);
                                        println!("{}", diff);
                                    }

                                    failed = true;
                                }
                            } else {
                                println!("{} (runtime)", Colour::Red.bold().paint("FAILED"));
                                failed_count += 1;

                                if !cmd.no_diff {
                                    let diff = prettydiff::diff_lines(&want, &got);
                                    println!("{}", diff);
                                }

                                failed = true;
                            }
                        }

                        if cmd.verbose {
                            println!("{:#}", err);
                        }

                        if failed && cmd.fail_early {
                            std::process::exit(1)
                        }
                    }
                }
            }
            Err(diagnostics) => {
                let mut failed = false;
                let mut formatter = Formatter::new(&test.source, diagnostics);
                if !test.skip {
                    let got = formatter.to_string().trim().to_owned();
                    let want = want.trim().to_owned();

                    if (test.result_approx && compare_partial_diagnostic(&got, &want))
                        || got == want
                    {
                        println!("{}", Colour::Green.bold().paint("OK"));
                    } else {
                        println!("{} (compilation)", Colour::Red.bold().paint("FAILED"));
                        failed_count += 1;

                        if !cmd.no_diff {
                            let diff = prettydiff::diff_lines(&want, &got);
                            println!("{}", diff);
                        }

                        failed = true;
                    }
                }

                if cmd.verbose {
                    formatter.enable_colors(true);
                    println!("{:#}", formatter);
                }

                if failed && cmd.fail_early {
                    std::process::exit(1)
                }
            }
        }
    }

    print_result(failed_count)
}

fn compare_partial_diagnostic(got: &str, want: &str) -> bool {
    got.lines()
        .filter(|line| line.trim().starts_with("error[E"))
        .zip(want.trim().lines())
        .all(|(got, want)| got.contains(want))
}

fn print_result(failed_count: usize) {
    let code = if failed_count > 0 { 1 } else { 0 };

    println!("\n");

    if failed_count > 0 {
        println!(
            "  Overall result: {}\n\n    Number failed: {}\n",
            Colour::Red.bold().paint("FAILED"),
            failed_count
        );
    } else {
        println!(
            "  Overall result: {}\n",
            Colour::Green.bold().paint("SUCCESS")
        );
    }

    std::process::exit(code)
}

fn vrl_value_to_json_value(value: Value) -> serde_json::Value {
    use serde_json::Value::*;

    match value {
        v @ Value::Bytes(_) => String(v.try_bytes_utf8_lossy().unwrap().into_owned()),
        Value::Integer(v) => v.into(),
        Value::Float(v) => v.into_inner().into(),
        Value::Boolean(v) => v.into(),
        Value::Object(v) => v
            .into_iter()
            .map(|(k, v)| (k, vrl_value_to_json_value(v)))
            .collect::<serde_json::Value>(),
        Value::Array(v) => v
            .into_iter()
            .map(vrl_value_to_json_value)
            .collect::<serde_json::Value>(),
        Value::Timestamp(v) => v.to_rfc3339_opts(SecondsFormat::AutoSi, true).into(),
        Value::Regex(v) => v.to_string().into(),
        Value::Null => Null,
    }
}
