#[path = "../examples/support/mod.rs"]
mod support;
use chrono::Utc;
use sqlx::types::Json;
use uuid::Uuid;
use ws_proxy::{
    api::models::*,
    registration::{NodeRuntimeStatus, RegistrationServerMessage},
    storage::{ControlStore, Mutation, PostgresStore},
};

fn host(id: Uuid) -> Host {
    Host {
        id,
        name: "database test".into(),
        ip: "127.0.0.1".parse().unwrap(),
        version: "test".into(),
        status: HostStatus::Online,
        last_seen_at: Utc::now(),
        metadata: None,
    }
}
fn update(name: &str) -> NodeUpdate {
    serde_json::from_value(serde_json::json!({"type":"display", "name":name, "enabled":true, "description":null, "settings":{"maxLines":3,"fontScale":1.0,"showOriginal":false,"theme":"dark"},"inputNodeId":null})).unwrap()
}

#[tokio::test]
#[ignore = "requires PostgreSQL; run just db-test"]
async fn durable_commands_concurrent_updates_sessions_and_notifications() {
    let database = support::Database::create().await.unwrap();
    let api = PostgresStore::connect(&database.url).await.unwrap();
    let proxy = PostgresStore::connect(&database.url).await.unwrap();
    let (host_id, session, node_id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    api.mutate(Mutation::Register(host(host_id), session))
        .await
        .unwrap();
    // The caller Gets and edits; Update directly submits those values. Reusing
    // a stale original must fail instead of silently merging or overwriting.
    let original = api.snapshot().await.unwrap();
    let mut edited = original.clone();
    edited.hosts.get_mut(&host_id).unwrap().name = "direct update".into();
    assert!(api.update(&original, &edited).await.unwrap());
    edited.hosts.get_mut(&host_id).unwrap().name = "stale overwrite".into();
    assert!(!api.update(&original, &edited).await.unwrap());
    assert_eq!(
        api.snapshot().await.unwrap().hosts[&host_id].name,
        "direct update"
    );

    let fixtures: NodeList =
        serde_json::from_str(include_str!("../../mockserver/api/v1/nodes.json")).unwrap();
    for node in fixtures.items {
        let Json(restored): Json<Node> = sqlx::query_scalar("SELECT $1::jsonb")
            .bind(Json(&node))
            .fetch_one(api.pool())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(&node).unwrap()
        );
        let Json(restored_host): Json<Host> = sqlx::query_scalar("SELECT $1::jsonb")
            .bind(Json(&node.base().host))
            .fetch_one(api.pool())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(restored_host).unwrap(),
            serde_json::to_value(&node.base().host).unwrap()
        );
    }
    let mut listener = sqlx::postgres::PgListener::connect(&database.url)
        .await
        .unwrap();
    listener.listen("control_changes").await.unwrap();
    let desired = update("screen");
    api.mutate(Mutation::Create(host_id, node_id, desired.clone()))
        .await
        .unwrap();
    let notification = tokio::time::timeout(std::time::Duration::from_secs(2), listener.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notification.payload(), host_id.to_string());
    let Json(restored): Json<NodeUpdate> =
        sqlx::query_scalar("SELECT desired FROM nodes WHERE id = $1")
            .bind(node_id)
            .fetch_one(api.pool())
            .await
            .unwrap();
    assert_eq!(restored, desired);
    assert!(matches!(restored.settings, NodeSettings::Display(_)));
    let Json(node): Json<Node> = sqlx::query_scalar("SELECT document FROM nodes WHERE id = $1")
        .bind(node_id)
        .fetch_one(api.pool())
        .await
        .unwrap();
    assert_eq!(node.id(), node_id);
    assert_eq!(proxy.pending(host_id, session).await.unwrap().len(), 1);
    // Socket delivery alone does not acknowledge; repeat reads replay the command.
    assert_eq!(
        proxy.pending(host_id, session).await.unwrap()[0].revision,
        1
    );
    proxy
        .mutate(Mutation::Status(
            host_id,
            session,
            node_id,
            1,
            NodeRuntimeStatus::Starting,
        ))
        .await
        .unwrap();
    assert_eq!(proxy.pending(host_id, session).await.unwrap().len(), 1);
    proxy
        .mutate(Mutation::Status(
            host_id,
            session,
            node_id,
            1,
            NodeRuntimeStatus::Stopped,
        ))
        .await
        .unwrap();
    assert_eq!(proxy.pending(host_id, session).await.unwrap().len(), 1);
    proxy
        .mutate(Mutation::Status(
            host_id,
            session,
            node_id,
            1,
            NodeRuntimeStatus::Running,
        ))
        .await
        .unwrap();
    proxy
        .mutate(Mutation::Status(
            host_id,
            session,
            node_id,
            1,
            NodeRuntimeStatus::Running,
        ))
        .await
        .unwrap();
    assert!(proxy.pending(host_id, session).await.unwrap().is_empty());

    let (a, b) = tokio::join!(
        api.mutate(Mutation::Update(node_id, update("edited"))),
        proxy.mutate(Mutation::Status(
            host_id,
            session,
            node_id,
            1,
            NodeRuntimeStatus::Failed
        ))
    );
    a.unwrap();
    b.unwrap();
    let state = api.snapshot().await.unwrap();
    assert_eq!(state.nodes[&node_id].revision, 2);
    assert_eq!(state.nodes[&node_id].node.base().name, "edited");
    assert_eq!(
        state.nodes[&node_id].node.status(),
        NodeStatus::Disconnected
    );
    let (a, b) = tokio::join!(
        api.mutate(Mutation::Restart(node_id)),
        proxy.mutate(Mutation::Restart(node_id))
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(api.snapshot().await.unwrap().nodes[&node_id].revision, 4);
    assert_eq!(
        proxy.pending(host_id, session).await.unwrap()[0].revision,
        2
    );
    // Each node is dispatched in revision order, irrespective of the latest revision.
    proxy
        .mutate(Mutation::Status(
            host_id,
            session,
            node_id,
            2,
            NodeRuntimeStatus::Running,
        ))
        .await
        .unwrap();
    assert_eq!(
        proxy.pending(host_id, session).await.unwrap()[0].revision,
        3
    );

    // A rolled-back write never notifies and leaves the revision unchanged.
    while tokio::time::timeout(std::time::Duration::from_millis(30), listener.recv())
        .await
        .is_ok()
    {}
    let mut transaction = api.pool().begin().await.unwrap();
    sqlx::query("UPDATE control_commands SET completed = true WHERE node_id = $1")
        .bind(node_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.recv())
            .await
            .is_err()
    );
    let mut invalid = desired.clone();
    invalid.base.name.clear();
    assert!(
        api.mutate(Mutation::Update(node_id, invalid))
            .await
            .is_err()
    );
    assert_eq!(api.snapshot().await.unwrap().nodes[&node_id].revision, 4);

    api.delete_completed_commands(host_id).await.unwrap();
    let completed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM control_commands WHERE completed")
            .fetch_one(api.pool())
            .await
            .unwrap();
    assert_eq!(completed, 0);
    assert_eq!(
        proxy.pending(host_id, session).await.unwrap()[0].revision,
        3
    );

    // Recover after a missed notification and recreate both store clients.
    drop(listener);
    // No listener observes this commit. A fresh store still scans durable work.
    api.mutate(Mutation::Health(
        host_id,
        session,
        "192.0.2.10".parse().unwrap(),
        None,
    ))
    .await
    .unwrap();
    drop(api);
    drop(proxy);
    let api = PostgresStore::connect(&database.url).await.unwrap();
    let proxy = PostgresStore::connect(&database.url).await.unwrap();
    assert_eq!(
        proxy.pending(host_id, session).await.unwrap()[0].revision,
        3
    );
    proxy
        .mutate(Mutation::Disconnect(host_id, session))
        .await
        .unwrap();
    assert_eq!(api.snapshot().await.unwrap().nodes.len(), 1);
    assert_eq!(
        api.snapshot().await.unwrap().nodes[&node_id]
            .node
            .base()
            .host
            .status,
        HostStatus::Offline
    );
    let next_session = Uuid::new_v4();
    proxy
        .mutate(Mutation::Register(host(host_id), next_session))
        .await
        .unwrap();
    assert!(
        proxy
            .mutate(Mutation::Disconnect(host_id, session))
            .await
            .is_err()
    );
    assert!(
        proxy
            .mutate(Mutation::Status(
                host_id,
                session,
                node_id,
                5,
                NodeRuntimeStatus::Running
            ))
            .await
            .is_err()
    );
    let pending = proxy.pending(host_id, next_session).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(
        matches!(&pending[0].message, RegistrationServerMessage::ApplyNode { revision: 5, update, .. } if update.base.name == "edited")
    );
    proxy
        .mutate(Mutation::Health(
            host_id,
            next_session,
            "192.0.2.1".parse().unwrap(),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        api.snapshot().await.unwrap().nodes[&node_id]
            .node
            .base()
            .host
            .ip
            .to_string(),
        "192.0.2.1"
    );
    // Expiry revokes a dead proxy's retained session and preserves configuration.
    sqlx::query("UPDATE hosts SET document = jsonb_set(document, '{lastSeenAt}', to_jsonb('2000-01-01T00:00:00Z'::text)) WHERE id = $1").bind(host_id).execute(api.pool()).await.unwrap();
    proxy.mutate(Mutation::Expire).await.unwrap();
    assert!(proxy.pending(host_id, next_session).await.is_err());
    assert_eq!(api.snapshot().await.unwrap().nodes.len(), 1);
    api.pool().close().await;
    proxy.pool().close().await;
}
