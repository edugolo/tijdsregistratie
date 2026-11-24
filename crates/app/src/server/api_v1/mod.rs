//! Public REST API v1 under `/api/v1`, separate from the browser's server functions.
//!
//! # Adding an endpoint
//!
//! 1. Write the handler in its own file here. Extract [`auth::ApiAuth`] first, call
//!    `auth.require(ApiScope::...)?` and, for site-scoped data, honour `auth.0.site_ids`
//!    (`ApiKey::allows_site`). Return `Result<Json<Dto>, error::ApiError>` and put the data
//!    logic in a plain function over the pool so it can be tested directly.
//! 2. Annotate it with `#[utoipa::path(..., path = "/api/v1/...", security(("bearer" = ["scope"])))]`
//!    (full path) and give DTOs `#[derive(Serialize, utoipa::ToSchema)]`.
//! 3. Register it with `.routes(routes!(module::handler))` in [`build`]. Authentication, the
//!    per-key rate limit and the `api.call` audit row come from the layer in [`auth`].
//!
//! Only the key management used by the Integraties page lives in [`keys`].

pub mod auth;
pub mod docs;
pub mod error;
pub mod keys;
pub mod people;

use axum::Extension;
use axum::Router;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{any, get};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use self::auth::ApiState;
use self::error::ApiError;
use super::AppState;

const DESCRIPTION: &str = "Read and sync data of one Tijdsregistratie deployment.\n\n\
Authenticate with an API key created under Instellingen > Integraties: \
`Authorization: Bearer tr_live_...`. Browser sessions and cookies are not accepted.\n\n\
Scopes: `people:read`, `people:write`, `hours:read`, `export:read`. A key can also be \
restricted to some sites. Each key is limited to 120 requests per minute (429, `Retry-After`). \
Errors are `{ \"error\": \"code\" }` with the codes `unauthorized` (401), `forbidden` (403), \
`not_found` (404), `rate_limited` (429), `invalid_request` (400) and `internal` (500). \
Keys give access to personal data: treat them like passwords.";

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("tr_live_<key>")
                    .description(Some(
                        "API key. Each endpoint lists the scope the key needs.",
                    ))
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Tijdsregistratie API", version = "1.0.0", description = DESCRIPTION),
    tags((name = "people", description = "People of the organisation")),
    components(schemas(error::ErrorBody, people::PersonDto)),
    modifiers(&SecurityAddon),
)]
struct ApiDoc;

async fn not_found() -> impl IntoResponse {
    ApiError::NotFound
}

fn build_parts() -> (Router, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(people::list_people))
        .split_for_parts()
}

/// The `/api/v1` router with the application state of the server.
pub fn router(state: AppState) -> Router {
    let docs = state.config.api_docs;
    build(ApiState::new(state), docs)
}

/// The `/api/v1` router. The API routes sit behind the key check; the reference and the OpenAPI
/// document are public (they describe the API, not the data) and absent when `docs` is false.
pub fn build(state: ApiState, docs: bool) -> Router {
    let (api, spec) = build_parts();
    let api = api
        .route("/api/v1", any(not_found))
        .route("/api/v1/{*rest}", any(not_found))
        .layer(Extension(state.clone()))
        .layer(middleware::from_fn_with_state(state, auth::authenticate));
    let reference = if docs {
        Router::new()
            .route(
                docs::OPENAPI_PATH,
                get(move || {
                    let spec = spec.clone();
                    async move { axum::Json(spec) }
                }),
            )
            .route(docs::DOCS_PATH, get(docs::page))
            .route(docs::SCRIPT_PATH, get(docs::script))
    } else {
        Router::new()
            .route(docs::OPENAPI_PATH, get(not_found))
            .route(docs::DOCS_PATH, get(not_found))
            .route(docs::SCRIPT_PATH, get(not_found))
    };
    api.merge(reference)
}

#[cfg(test)]
mod tests;
