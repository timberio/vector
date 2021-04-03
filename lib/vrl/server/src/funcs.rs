use std::convert::Infallible;
use warp::{reply::json, Reply};

pub async fn function_metadata() -> Result<impl Reply, Infallible> {
    let functions: Vec<&'static str> = stdlib::all().iter().map(|f| f.identifier()).collect();
    Ok(json(&functions))
}
