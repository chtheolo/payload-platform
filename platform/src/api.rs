use axum::routing::{get, post};
use axum::{Json, Router};
use axum::extract::{Path, State};

use crate::payload::PayloadHandle;

pub fn router(payload: PayloadHandle) -> Router {
    Router::new()
        .route("/v1/payload", get(get_payload))
        .route(
            "/v1/payload/properties/{id}",
            get(get_property).put(put_property),
        )
}

/// `GEt /v1/payload`: everything GCS can know from the latest snapshot
async fn get_payload(State(payload): State<PayloadHandle>) -> Json<Snapshot> {
    Json(payload.snapshot())
}

/// `GET /v1/payload/properties/{id}`:
async fn get_property(
    State(payload): State<PayloadHandle>,
    Path(id): Path<String>,
) -> Result<Json<Property, PayloadError>> {

}
