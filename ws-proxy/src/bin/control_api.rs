use actix_web::{App, HttpServer};
use utoipa_actix_web::AppExt;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let url = ws_proxy::storage::postgres::database_url()?;
    let store: ws_proxy::storage::Store =
        std::sync::Arc::new(ws_proxy::storage::PostgresStore::connect(&url).await?);
    eprintln!("Suntitle REST API listening on 0.0.0.0:3031");
    HttpServer::new(move || {
        let store = store.clone();
        App::new()
            .into_utoipa_app()
            .configure(move |c| ws_proxy::api::configure_with_store(c, store))
            .into_app()
    })
    .bind(("0.0.0.0", 3031))?
    .run()
    .await
}
