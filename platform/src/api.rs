/// The HTTP layer for GCS commands

use axum::extract::rejection::JsonRejection;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum::extract::{Path, State};

use crate::payload::PayloadHandle;
use crate::error::PayloadError;

pub fn router(payload: PayloadHandle) -> Router {
    Router::new()
        .route("/v1/health", get(health_status))
        .route("/v1/payload", get(get_payload))
        .route(
            "/v1/payload/properties/{id}",
            get(get_property).put(put_property),
        )
        .route("/v1/payload/actions/{id}",post(post_action))
}


async fn health_status(State(payload): State<PayloadHandle>) -> Json<String> {
    Json(json!({"status":"running"}))
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
) -> Result<Json<Property, PayloadError>> {
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
// async fn post_action(
//     State(payload): State<PayloadHandle>,
//     Path(id): Path<String>,
//     body: Result<Json<BTreeMapM<String, Value>>, JsonRejection>,
// ) -> Result<Json<, PayloadError> {
//     let Json(arguments) = body.map_err(invalid_body)?;
//     let properties = payload.invoke(&id, )


// }