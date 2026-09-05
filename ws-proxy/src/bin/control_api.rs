use actix_web::{App, HttpServer};
use utoipa_actix_web::AppExt;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    eprintln!("Suntitle REST API listening on 0.0.0.0:3031 with a process-local host registry");
    HttpServer::new(|| {
        App::new()
            .into_utoipa_app()
            .configure(ws_proxy::api::configure)
            .into_app()
    })
    .bind(("0.0.0.0", 3031))?
    .run()
    .await
}
