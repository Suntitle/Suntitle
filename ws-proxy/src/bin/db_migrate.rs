#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = ws_proxy::storage::postgres::database_url()?;
    ws_proxy::storage::postgres::migrate(&url).await
}
