use sqlx::{Connection, Executor};

#[allow(dead_code)]
pub fn require_free_ports() -> std::io::Result<()> {
    let _proxy = std::net::TcpListener::bind("127.0.0.1:3030")?;
    let _api = std::net::TcpListener::bind("127.0.0.1:3031")?;
    Ok(())
}

/// Each E2E run uses its own schema, preserving local development data.
pub struct Database {
    pub url: String,
    base_url: String,
    schema: String,
}
impl Database {
    pub async fn create() -> Result<Self, Box<dyn std::error::Error>> {
        let base_url = ws_proxy::storage::postgres::database_url()?;
        let schema = format!("e2e_{}", uuid::Uuid::new_v4().simple());
        let mut connection = sqlx::PgConnection::connect(&base_url).await?;
        connection
            .execute(format!("CREATE SCHEMA {schema}").as_str())
            .await?;
        let mut url = url::Url::parse(&base_url)?;
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let database = Self {
            url: url.to_string(),
            base_url,
            schema,
        };
        let pool = sqlx::PgPool::connect(&database.url).await?;
        ws_proxy::storage::postgres::MIGRATOR.run(&pool).await?;
        pool.close().await;
        Ok(database)
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let url = self.base_url.clone();
        let schema = self.schema.clone();
        // Child process guards are dropped first. A separate runtime permits
        // synchronous RAII cleanup on both successful and failed E2E runs.
        let _ = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    if let Ok(mut connection) = sqlx::PgConnection::connect(&url).await {
                        let _ = connection
                            .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
                            .await;
                    }
                });
        })
        .join();
    }
}
