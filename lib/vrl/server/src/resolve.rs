use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use vrl::{diagnostic::Formatter, state, Runtime, Value};
use warp::{reply::json, Reply};

#[derive(Deserialize, Serialize)]
pub struct Input {
    program: String,
    event: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success { output: Value, result: Value },
    Error { error: String },
}

pub async fn resolve_vrl_input(input: Input) -> Result<impl Reply, Infallible> {
    let outcome = resolve(input);
    Ok(json(&outcome))
}

fn resolve(mut input: Input) -> Outcome {
    let event = &mut input.event;
    let mut state = state::Compiler::default();
    let mut runtime = Runtime::new(state::Runtime::default());

    let program = match vrl::compile_with_state(&input.program, &stdlib::all(), &mut state) {
        Ok(program) => program,
        Err(diagnostics) => {
            let msg = Formatter::new(&input.program, diagnostics).to_string();
            return Outcome::Error { error: msg };
        }
    };

    match runtime.resolve(event, &program) {
        Ok(result) => Outcome::Success {
            output: result,
            result: event.clone(),
        },
        Err(err) => Outcome::Error {
            error: err.to_string(),
        },
    }
}
