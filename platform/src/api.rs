/// The HTTP layer for GCS commands

use std::collections::BTreeMap;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::PayloadError;
use crate::model::{Property, Reading, Snapshot, Value};
use crate::payload::PayloadHandle;
use crate::telemetry;

pub fn router(payload: PayloadHandle) -> Router {
    let router = Router::new()
        .route("/v1/payload", get(get_payload))
        .route(
            "/v1/payload/properties/{id}",
            get(get_property).put(put_property),
        )
        .route("/v1/payload/actions/{id}", post(post_action))
        // Unknown paths get a JSON error too, instead of an empty 404.
        .fallback(unknown_path)
        .with_state(payload);
    // counts and times every request in the development build; unchanged in production
    telemetry::instrument(router)
}


/// `GEt /v1/payload`: everything GCS can know from the latest snapshot
async fn get_payload(State(payload): State<PayloadHandle>) -> Json<Snapshot> {
    Json(payload.snapshot())
}

/// `GET /v1/payload/properties/{id}`:
///  properties: { "sensor": …, "thermal_palette": …, "zoom": … }
/// then remove("zoom") moved out, no copying
/// zoom -> Json -> return to the GCS
async fn get_property(
    State(payload): State<PayloadHandle>,
    Path(id): Path<String>,
) -> Result<Json<Property>, PayloadError> {
    payload
        .snapshot()
        .properties
        .remove(&id)
        .map(Json)
        .ok_or_else(|| PayloadError::NotFound(format!("this payload has no property `{id}`")))
    
    // the alternative of remove(&id) that also works would be 
    // that's correct too, but it makes a second copy of a property 
    // that we already copied once in snapshot().
    // payload
    //     .snapshot()
    //     .properties
    //     .get(&id).cloned()
}

/// The body of `PUT /v1/payload/properties/{id}`: `{"value": ...}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteBody {
    value: Value,
}

/// `PUT /v1/payload/properties/{id}`
/// e.g. curl -X PUT localhost:8080/v1/payload/properties/zoom --json '{"value": 10.5}' 
async fn put_property(
    State(payload): State<PayloadHandle>,
    Path(id): Path<String>,
    body: Result<Json<WriteBody>, JsonRejection>,
) -> Result<Json<Property>, PayloadError> {
    let Json(body) = body.map_err(invalid_body)?;
    let property = payload.write(&id, body.value).await?;
    Ok(Json(property))
}

/// The answer to an action: the properties it changed.
#[derive(Serialize)]
struct ActionReply {
    properties: BTreeMap<String, Reading>,
}

/// `POST /v1/payload/actions/{id}`; the body holds the arguments, e.g. `{"direction": "in"}`.
async fn post_action(
    State(payload): State<PayloadHandle>,
    Path(id): Path<String>,
    body: Result<Json<BTreeMap<String, Value>>, JsonRejection>,
) -> Result<Json<ActionReply>, PayloadError> {
    let Json(arguments) = body.map_err(invalid_body)?;
    let properties = payload.invoke(&id, arguments).await?;
    Ok(Json(ActionReply { properties }))
}

fn invalid_body(rejection: JsonRejection) -> PayloadError {
    PayloadError::InvalidRequest(rejection.body_text())
}

async fn unknown_path() -> PayloadError {
    PayloadError::NotFound("no such path; start with GET /v1/payload".into())
}

/// How each `PayloadError` looks over HTTP: a status code plus error message
/// `{"error": {"code", "message", "details"}}`.
impl IntoResponse for PayloadError {
    fn into_response(self) -> Response {
        telemetry::error_sent(self.code());
        let status = match &self {
            PayloadError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            PayloadError::NotFound(_) => StatusCode::NOT_FOUND,
            PayloadError::ReadOnly(_) => StatusCode::METHOD_NOT_ALLOWED,
            PayloadError::UnsupportedValue { .. } | PayloadError::OutOfRange { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            PayloadError::Rejected(_) => StatusCode::CONFLICT,
            PayloadError::PayloadOffline | PayloadError::Busy => StatusCode::SERVICE_UNAVAILABLE,
            PayloadError::Timeout => StatusCode::GATEWAY_TIMEOUT,
            PayloadError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let mut error = json!({ "code": self.code(), "message": self.to_string() });
        if let Some(details) = self.details() {
            error["details"] = details;
        }
        (status, Json(json!({ "error": error }))).into_response()
    }
}