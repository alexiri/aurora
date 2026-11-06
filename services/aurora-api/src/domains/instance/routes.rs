use super::service::{InstanceInfo, instance_info};
use crate::app::AppState;
use crate::http::error::ErrorEnvelope;
use crate::http::json::Json;
use axum::extract::State;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_instance))
}

/// Public instance information (no authentication).
#[utoipa::path(
    get,
    path = "/instance",
    operation_id = "getInstance",
    tag = "instance",
    responses(
        (status = 200, description = "Instance information", body = InstanceInfo),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
async fn get_instance(State(state): State<AppState>) -> Json<InstanceInfo> {
    Json(instance_info(&state.config))
}
