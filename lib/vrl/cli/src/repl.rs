use super::common::{open_url, print_function_list, Repl};
use indoc::indoc;
use lazy_static::lazy_static;
use regex::Regex;
use rustyline::{error::ReadlineError, Editor};
use vrl::{diagnostic::Formatter, state, Runtime, Target, Value};

// Create a list of all possible error values for potential docs lookup
lazy_static! {
    static ref ERRORS: Vec<String> = [
        100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 203, 204, 205, 206, 207, 208, 209,
        601, 300, 301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314, 400, 401,
        601, 620, 630, 640, 650, 660
    ]
    .iter()
    .map(|i| i.to_string())
    .collect();
}

const DOCS_URL: &str = "https://vrl.dev";
const ERRORS_URL_ROOT: &str = "https://errors.vrl.dev";

pub(crate) fn run(mut objects: Vec<Value>) {
    let mut index = 0;
    let func_docs_regex = Regex::new(r"^help\sdocs\s(\w{1,})$").unwrap();
    let error_docs_regex = Regex::new(r"^help\serror\s(\w{1,})$").unwrap();

    let mut compiler_state = state::Compiler::default();
    let mut rt = Runtime::new(state::Runtime::default());
    let mut rl = Editor::<Repl>::new();
    rl.set_helper(Some(Repl::new("$ ")));

    println!("{}", BANNER_TEXT);

    loop {
        let readline = rl.readline("$ ");
        match readline.as_deref() {
            Ok(line) if line == "exit" || line == "quit" => break,
            Ok(line) if line == "help" => print_help_text(),
            Ok(line) if line == "help functions" || line == "help funcs" || line == "help fs" => {
                print_function_list()
            }
            Ok(line) if line == "help docs" => open_url(DOCS_URL),
            // Capture "help error <code>"
            Ok(line) if error_docs_regex.is_match(line) => show_error_docs(line, &error_docs_regex),
            // Capture "help docs <func_name>"
            Ok(line) if func_docs_regex.is_match(line) => show_func_docs(line, &func_docs_regex),
            Ok(line) => {
                rl.add_history_entry(line);

                let command = match line {
                    "next" => {
                        // allow adding one new object at a time
                        if index < objects.len() && objects.last() != Some(&Value::Null) {
                            index = index.saturating_add(1);
                        }

                        // add new object
                        if index == objects.len() {
                            objects.push(Value::Null)
                        }

                        "."
                    }
                    "prev" => {
                        index = index.saturating_sub(1);

                        // remove empty last object
                        if objects.last() == Some(&Value::Null) {
                            let _ = objects.pop();
                        }

                        "."
                    }
                    "" => continue,
                    _ => line,
                };

                let value = resolve_to_string(
                    objects.get_mut(index),
                    &mut rt,
                    command,
                    &mut compiler_state,
                );
                println!("{}\n", value);
            }
            Err(ReadlineError::Interrupted) => break,
            Err(ReadlineError::Eof) => break,
            Err(err) => {
                println!("unable to read line: {}", err);
                break;
            }
        }
    }
}

fn resolve_to_string(
    object: Option<&mut impl Target>,
    runtime: &mut Runtime,
    program: &str,
    state: &mut state::Compiler,
) -> String {
    let object = match object {
        None => return Value::Null.to_string(),
        Some(object) => object,
    };

    let program = match vrl::compile_with_state(program, &stdlib::all(), state) {
        Ok(program) => program,
        Err(diagnostics) => return Formatter::new(program, diagnostics).colored().to_string(),
    };

    match runtime.resolve(object, &program) {
        Ok(value) => value.to_string(),
        Err(err) => err.to_string(),
    }
}

fn print_help_text() {
    println!("{}", HELP_TEXT);
}

fn show_func_docs(line: &str, pattern: &Regex) {
    // Unwrap is okay in both cases here, as there's guaranteed to be two matches ("help docs" and
    // "help docs <func_name>")
    let matches = pattern.captures(line).unwrap();
    let func_name = matches.get(1).unwrap().as_str();

    if stdlib::all().iter().any(|f| f.identifier() == func_name) {
        let func_url = format!("{}/functions/#{}", DOCS_URL, func_name);
        open_url(&func_url);
    } else {
        println!("function name {} not recognized", func_name);
    }
}

fn show_error_docs(line: &str, pattern: &Regex) {
    // As in show_func_docs, unwrap is okay here
    let matches = pattern.captures(line).unwrap();
    let error_code = matches.get(1).unwrap().as_str();

    if ERRORS.iter().any(|e| e == error_code) {
        let error_code_url = format!("{}/{}", ERRORS_URL_ROOT, error_code);
        open_url(&error_code_url);
    } else {
        println!("error code {} not recognized", error_code);
    }
}

const HELP_TEXT: &str = indoc! {r#"
    VRL REPL commands:
      .                  Show the current value of the event
      help functions     Display a list of currently available VRL functions (aliases: ["help funcs", "help fs"])
      help docs          Navigate to the VRL docs on the Vector website
      help docs <func>   Navigate to the VRL docs for the specified function
      help error <code>  Navigate to the docs for a specific error code
      next               Load the next object or create a new one
      prev               Load the previous object
      exit               Terminate the program
"#};

const BANNER_TEXT: &str = indoc! {r#"
    > VVVVVVVV           VVVVVVVVRRRRRRRRRRRRRRRRR   LLLLLLLLLLL
    > V::::::V           V::::::VR::::::::::::::::R  L:::::::::L
    > V::::::V           V::::::VR::::::RRRRRR:::::R L:::::::::L
    > V::::::V           V::::::VRR:::::R     R:::::RLL:::::::LL
    >  V:::::V           V:::::V   R::::R     R:::::R  L:::::L
    >   V:::::V         V:::::V    R::::R     R:::::R  L:::::L
    >    V:::::V       V:::::V     R::::RRRRRR:::::R   L:::::L
    >     V:::::V     V:::::V      R:::::::::::::RR    L:::::L
    >      V:::::V   V:::::V       R::::RRRRRR:::::R   L:::::L
    >       V:::::V V:::::V        R::::R     R:::::R  L:::::L
    >        V:::::V:::::V         R::::R     R:::::R  L:::::L
    >         V:::::::::V          R::::R     R:::::R  L:::::L         LLLLLL
    >          V:::::::V         RR:::::R     R:::::RLL:::::::LLLLLLLLL:::::L
    >           V:::::V          R::::::R     R:::::RL::::::::::::::::::::::L
    >            V:::V           R::::::R     R:::::RL::::::::::::::::::::::L
    >             VVV            RRRRRRRR     RRRRRRRLLLLLLLLLLLLLLLLLLLLLLLL
    >
    >                     VECTOR    REMAP    LANGUAGE
    >
    >
    > Welcome!
    >
    > The CLI is running in REPL (Read-eval-print loop) mode.
    >
    > To run the CLI in regular mode, add a program to your command.
    >
    > VRL REPL commands:
    >   .                 Show the current value of the event
    >   help              Learn more about VRL
    >   next              Load the next object or create a new one
    >   prev              Load the previous object
    >   exit              Terminate the program
    >
    > Any other value is resolved to a VRL expression.
    >
    > Try it out now by typing `.` and hitting [enter] to see the result.
"#};
