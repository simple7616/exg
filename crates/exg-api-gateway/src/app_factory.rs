use actix_web::{App, web};

use crate::handlers::{amend_order, cancel_order, health, login, me, place_order, register};
use crate::state::AppState;

/// Build the Actix `App` with the Stage 1a route table.
///
/// Returns an opaque `App` so the caller can do:
/// ```ignore
/// HttpServer::new(move || build_app(state.clone()))
/// ```
pub fn build_app(
    state: AppState,
) -> App<
    impl actix_web::dev::ServiceFactory<
        actix_web::dev::ServiceRequest,
        Config = (),
        Response = actix_web::dev::ServiceResponse<actix_web::body::BoxBody>,
        Error = actix_web::Error,
        InitError = (),
    >,
> {
    App::new()
        .app_data(web::Data::new(state))
        .route("/api/v1/health", web::get().to(health))
        .route("/api/v1/order", web::post().to(place_order))
        .route("/api/v1/order/cancel", web::post().to(cancel_order))
        .route("/api/v1/order/amend", web::post().to(amend_order))
        .route("/api/v1/auth/register", web::post().to(register))
        .route("/api/v1/auth/login", web::post().to(login))
        .route("/api/v1/me", web::get().to(me))
}
