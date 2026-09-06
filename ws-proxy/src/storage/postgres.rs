use super::*;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, Value};
use sea_query_binder::SqlxBinder;
use sqlx::{PgConnection, PgPool, Row, postgres::PgPoolOptions, types::Json};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub fn database_url() -> std::io::Result<String> {
    // An explicit path makes invocation from either the repository or crate work.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.env.local");
    match dotenvy::from_path(path) {
        Ok(_) => {}
        Err(dotenvy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(std::io::Error::other(e)),
    }
    std::env::var("DATABASE_URL").map_err(|_| {
        std::io::Error::other("DATABASE_URL is required; copy .env.example to .env.local")
    })
}

fn storage_error(error: impl std::fmt::Display) -> ApiError {
    eprintln!("control storage error: {error}");
    ApiError::problem(
        actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
        "Storage unavailable",
        "control storage operation failed",
    )
}

#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}
impl PostgresStore {
    pub async fn connect(url: &str) -> std::io::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await
            .map_err(std::io::Error::other)?;
        let rows: Vec<(i64, bool, Vec<u8>)> = sqlx::query_as(
            "SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .map_err(std::io::Error::other)?;
        let expected: Vec<_> = MIGRATOR.iter().collect();
        if rows.len() != expected.len()
            || rows
                .iter()
                .zip(expected)
                .any(|((version, success, checksum), migration)| {
                    *version != migration.version
                        || !success
                        || checksum.as_slice() != migration.checksum.as_ref()
                })
        {
            return Err(std::io::Error::other(
                "incompatible control schema; run just db-migrate",
            ));
        }
        Ok(Self { pool })
    }
    /// Subscribes before returning to the proxy. SQL connections, the single
    /// proxy lease, LISTEN and reconnect handling stay inside this adapter.
    pub async fn proxy_events(&self, url: &str) -> std::io::Result<ProxyEvents> {
        let mut lease = self.pool.acquire().await.map_err(std::io::Error::other)?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(30313031)")
            .fetch_one(&mut *lease)
            .await
            .map_err(std::io::Error::other)?;
        if !acquired {
            return Err(std::io::Error::other("another ws-proxy is already active"));
        }
        let mut listener = sqlx::postgres::PgListener::connect(url)
            .await
            .map_err(std::io::Error::other)?;
        listener
            .listen("control_changes")
            .await
            .map_err(std::io::Error::other)?;
        let (changes, receiver) = tokio::sync::watch::channel(0_u64);
        let store = self.clone();
        let task = tokio::spawn(async move {
            let mut recovery = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tokio::select! {
                    result = listener.recv() => {
                        if let Err(error) = result {
                            eprintln!("control listener reconnecting: {error}");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                    }
                    _ = recovery.tick() => {
                        // Losing the lease connection stops the proxy, avoiding
                        // two dispatchers if another instance acquires the lease.
                        sqlx::query("SELECT 1").execute(&mut *lease).await.map_err(std::io::Error::other)?;
                        if let Err(error) = store.mutate(Mutation::Expire).await { eprintln!("host expiration failed: {error}"); }
                    }
                }
                changes.send_modify(|version| *version = version.wrapping_add(1));
            }
        });
        Ok(ProxyEvents { receiver, task })
    }
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

pub struct ProxyEvents {
    pub receiver: tokio::sync::watch::Receiver<u64>,
    pub task: tokio::task::JoinHandle<std::io::Result<()>>,
}

pub async fn migrate(url: &str) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    MIGRATOR.run(&pool).await?;
    pool.close().await;
    Ok(())
}

async fn read(connection: &mut PgConnection) -> Result<Snapshot> {
    let query = Query::select()
        .column(Alias::new("version"))
        .from(Alias::new("control_version"))
        .to_string(PostgresQueryBuilder);
    let mut state = Snapshot {
        version: sqlx::query_scalar(&query)
            .fetch_one(&mut *connection)
            .await
            .map_err(storage_error)?,
        ..Snapshot::default()
    };
    let query = Query::select()
        .column(sea_query::Asterisk)
        .from(Alias::new("hosts"))
        .to_string(PostgresQueryBuilder);
    for row in sqlx::query(&query)
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?
    {
        let Json(host): Json<Host> = row.try_get("document").map_err(storage_error)?;
        if let Some(session) = row
            .try_get::<Option<Uuid>, _>("session_id")
            .map_err(storage_error)?
        {
            state.sessions.insert(host.id, session);
        }
        state.hosts.insert(host.id, host);
    }
    let query = Query::select()
        .column(sea_query::Asterisk)
        .from(Alias::new("nodes"))
        .to_string(PostgresQueryBuilder);
    for row in sqlx::query(&query)
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?
    {
        let Json(mut node): Json<Node> = row.try_get("document").map_err(storage_error)?;
        let host_id = row.try_get("host_id").map_err(storage_error)?;
        node.base_mut().host = state
            .hosts
            .get(&host_id)
            .cloned()
            .ok_or_else(|| storage_error("missing node host"))?;
        let Json(update): Json<NodeUpdate> = row.try_get("desired").map_err(storage_error)?;
        state.nodes.insert(
            node.id(),
            DesiredNode {
                host_id,
                revision: row.try_get::<i64, _>("revision").map_err(storage_error)? as u64,
                update,
                node,
            },
        );
    }
    let query = Query::select()
        .column(sea_query::Asterisk)
        .from(Alias::new("control_commands"))
        .and_where(Expr::col(Alias::new("completed")).eq(false))
        .to_string(PostgresQueryBuilder);
    for row in sqlx::query(&query)
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?
    {
        let Json(message) = row.try_get("message").map_err(storage_error)?;
        let command = PendingCommand {
            id: row.try_get("id").map_err(storage_error)?,
            host_id: row.try_get("host_id").map_err(storage_error)?,
            node_id: row.try_get("node_id").map_err(storage_error)?,
            revision: row.try_get::<i64, _>("revision").map_err(storage_error)? as u64,
            message,
            completed: false,
        };
        state.commands.insert(command.id, command);
    }
    Ok(state)
}

/// SQL writes take supplied values directly; none performs a Get or merges data.
async fn create(
    connection: &mut PgConnection,
    table: &str,
    columns: &[&str],
    values: Vec<Value>,
) -> Result<()> {
    let (sql, values) = Query::insert()
        .into_table(Alias::new(table))
        .columns(columns.iter().map(|c| Alias::new(*c)))
        .values_panic(values.into_iter().map(Into::into))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .execute(connection)
        .await
        .map_err(storage_error)?;
    Ok(())
}
async fn update(
    connection: &mut PgConnection,
    table: &str,
    columns: &[&str],
    values: Vec<Value>,
) -> Result<()> {
    let mut query = Query::update();
    query
        .table(Alias::new(table))
        .and_where(Expr::col(Alias::new(columns[0])).eq(values[0].clone()));
    for (column, value) in columns[1..].iter().zip(values.into_iter().skip(1)) {
        query.value(Alias::new(*column), value);
    }
    let (sql, values) = query.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .execute(connection)
        .await
        .map_err(storage_error)?;
    Ok(())
}
async fn save(
    connection: &mut PgConnection,
    exists: bool,
    table: &str,
    columns: &[&str],
    values: Vec<Value>,
) -> Result<()> {
    if exists {
        update(connection, table, columns, values).await
    } else {
        create(connection, table, columns, values).await
    }
}
fn json(value: &impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value).map_err(storage_error)?.into())
}
fn changed(a: &impl Serialize, b: Option<&impl Serialize>) -> bool {
    b.is_none_or(|b| serde_json::to_value(a).ok() != serde_json::to_value(b).ok())
}

#[async_trait]
impl ControlStore for PostgresStore {
    async fn snapshot(&self) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        let state = read(&mut tx).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(state)
    }
    async fn update(&self, old: &Snapshot, next: &Snapshot) -> Result<bool> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        // This is a direct conditional Update, not a read-modify-write. Callers
        // already loaded and modified domain models. A conflict rolls back all
        // writes and lets the caller repeat Get before modifying fresh models.
        let (sql, values) = Query::update()
            .table(Alias::new("control_version"))
            .value(Alias::new("version"), old.version + 1)
            .and_where(Expr::col(Alias::new("version")).eq(old.version))
            .build_sqlx(PostgresQueryBuilder);
        if sqlx::query_with(&sql, values)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected()
            == 0
        {
            tx.rollback().await.map_err(storage_error)?;
            return Ok(false);
        }
        for (id, host) in &next.hosts {
            if changed(host, old.hosts.get(id)) || next.sessions.get(id) != old.sessions.get(id) {
                save(
                    &mut tx,
                    old.hosts.contains_key(id),
                    "hosts",
                    &["id", "document", "session_id"],
                    vec![
                        (*id).into(),
                        json(host)?,
                        next.sessions.get(id).copied().into(),
                    ],
                )
                .await?;
            }
        }
        for (id, node) in &next.nodes {
            if changed(node, old.nodes.get(id)) {
                save(
                    &mut tx,
                    old.nodes.contains_key(id),
                    "nodes",
                    &["id", "host_id", "revision", "document", "desired"],
                    vec![
                        (*id).into(),
                        node.host_id.into(),
                        (node.revision as i64).into(),
                        json(&node.node)?,
                        json(&node.update)?,
                    ],
                )
                .await?;
            }
        }
        for (id, command) in &next.commands {
            if changed(command, old.commands.get(id)) {
                save(
                    &mut tx,
                    old.commands.contains_key(id),
                    "control_commands",
                    &[
                        "id",
                        "host_id",
                        "node_id",
                        "revision",
                        "message",
                        "completed",
                    ],
                    vec![
                        (*id).into(),
                        command.host_id.into(),
                        command.node_id.into(),
                        (command.revision as i64).into(),
                        json(&command.message)?,
                        command.completed.into(),
                    ],
                )
                .await?;
            }
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(true)
    }
    async fn delete_completed_commands(&self, host_id: Uuid) -> Result<()> {
        let (sql, values) = Query::delete()
            .from_table(Alias::new("control_commands"))
            .and_where(Expr::col(Alias::new("host_id")).eq(host_id))
            .and_where(Expr::col(Alias::new("completed")).eq(true))
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values)
            .execute(&self.pool)
            .await
            .map_err(storage_error)?;
        Ok(())
    }
}
