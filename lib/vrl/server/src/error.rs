use http::status::StatusCode;
use serde::Serialize;
use std::convert::Infallible;
use warp::filters::body::BodyDeserializeError;
use warp::reject::{MethodNotAllowed, Rejection};
use warp::reply::{json, with_status as status, Json, Reply};

#[derive(Serialize)]
struct Error {
    error: String,
}

impl Error {
    fn new(msg: &str) -> Self {
        Self {
            error: msg.to_owned(),
        }
    }

    fn not_found() -> Json {
        Self::new("not found").as_json()
    }

    fn body_deserialization(err: &BodyDeserializeError) -> Json {
        Self::new(&err.to_string()).as_json()
    }

    fn unknown() -> Json {
        Self::new("unknown").as_json()
    }

    fn method_not_allowed() -> Json {
        Self::new("method not allowed").as_json()
    }

    fn as_json(&self) -> Json {
        json(self)
    }
}

pub async fn handle_err(err: Rejection) -> Result<impl Reply, Infallible> {
    let result = if err.is_not_found() {
        status(Error::not_found(), StatusCode::NOT_FOUND)
    } else if let Some(_) = err.find::<MethodNotAllowed>() {
        status(Error::method_not_allowed(), StatusCode::METHOD_NOT_ALLOWED)
    } else if let Some(e) = err.find::<BodyDeserializeError>() {
        status(Error::body_deserialization(e), StatusCode::BAD_REQUEST)
    } else {
        status(Error::unknown(), StatusCode::INTERNAL_SERVER_ERROR)
    };

    Ok(result)
}
