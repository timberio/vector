extern crate lalrpop;

use std::fmt::Write as fmt_write;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::{env, fs};

fn main() {
    lalrpop::Configuration::new()
        .always_use_colors()
        .process_current_dir()
        .unwrap();

    println!("cargo:rerun-if-changed=src/parser.lalrpop");

    read_grok_patterns();
}

/// Reads grok patterns defined in the `patterns` folder into the static `PATTERNS` variable
fn read_grok_patterns() {
    let mut output = "static PATTERNS: &[(&str, &str)] = &[\n".to_string();

    fs::read_dir(Path::new("patterns"))
        .expect("can read 'patterns' dir")
        .filter_map(|path| File::open(path.expect("can read 'patterns' dir").path()).ok())
        .flat_map(|f| BufReader::new(f).lines().filter_map(|l| l.ok()))
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .for_each(|line| {
            let (key, value) =
                line.split_at(line.find(' ').expect("pattern is 'ruleName definition'"));
            write!(output, "\t(\"{}\", r#\"{}\"#),", key, &value[1..])
                .expect("can append patterns");
        });

    output.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is defined");
    let dest_path = Path::new(&out_dir).join("patterns.rs");
    fs::write(dest_path, output).expect("'patterns.rs' is created");
}
