use futures_util::FutureExt;
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, Patch, PatchParams, PostParams},
};
use serde_json::json;
use std::{collections::BTreeMap, panic::AssertUnwindSafe, path::Path, time::Duration};
use suntitle_core::{
    kubernetes::{client_from_kubeconfig, resources::*},
    registration::*,
};
use ws_proxy::controller::{Connection, Controller};

fn wire<T: serde::Serialize + serde::de::DeserializeOwned>(message: T) -> T {
    let bytes = serde_json::to_vec(&Envelope::new(message)).unwrap();
    let decoded: Envelope<T> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded.protocol_version, 1);
    decoded.message
}

async fn connect(controller: &Controller, name: &str) -> Connection {
    controller
        .register(RegistrationClientMessage::Register {
            host_name: name.into(),
            agent_version: "test".into(),
            reported_metadata: BTreeMap::new(),
            ip: None,
        })
        .await
        .unwrap()
}

/// A simulated host MUST acknowledge OCI preparation before the controller dispatches apply.
async fn apply(controller: &Controller, connection: &mut Connection) -> ApplyIdentity {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(15), connection.outbound.recv())
            .await
            .unwrap()
            .unwrap();
        match wire(message) {
            RegistrationServerMessage::DownloadArtifact {
                identity,
                reference,
                ..
            } => {
                assert!(!reference.is_empty());
                controller
                    .report(
                        connection,
                        wire(RegistrationClientMessage::Acknowledgement {
                            identity,
                            operation: Operation::Download,
                            success: true,
                            message: None,
                        }),
                    )
                    .await;
            }
            RegistrationServerMessage::Apply {
                identity, custom, ..
            } => {
                assert!(
                    custom.is_some(),
                    "every apply MUST reference a prepared artifact"
                );
                return identity;
            }
            other => panic!("expected download or apply, got {other:?}"),
        }
    }
}

/// Wait for the acknowledged apply phase; stale reports MUST leave the current operation unchanged.
async fn ack(
    nodes: &Api<SuntitleNode>,
    controller: &Controller,
    connection: &Connection,
    identity: ApplyIdentity,
    success: bool,
) {
    controller
        .report(
            connection,
            wire(RegistrationClientMessage::Acknowledgement {
                identity: identity.clone(),
                operation: Operation::Apply,
                success,
                message: None,
            }),
        )
        .await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let Some(node) = nodes.get_opt(&identity.target.name).await.unwrap() else {
                break;
            };
            let Some(dispatch) = node
                .status
                .as_ref()
                .and_then(|status| status.dispatch.as_ref())
            else {
                break;
            };
            if dispatch.operation_id != identity.operation_id
                || dispatch.operation != Operation::Apply
                || dispatch.success != Some(success)
            {
                break;
            }
            let status = node.status.unwrap();
            if (!success && matches!(status.phase, Some(NodePhase::Failed)))
                || (success
                    && status.observed_generation == Some(identity.generation)
                    && status.reporting_session.as_deref() == Some(&identity.session_id)
                    && matches!(status.phase, Some(NodePhase::Running | NodePhase::Disabled)))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

async fn quiet(connection: &mut Connection) {
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), connection.outbound.recv())
            .await
            .is_err()
    );
}

/// Select explicit isolated test credentials or the normal bootstrapped proxy credentials.
async fn test_client() -> kube::Client {
    let path = std::env::var_os("TEST_PROXY_KUBECONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("pki/suntitle-proxy.kubeconfig")
        });
    client_from_kubeconfig(&path).await.unwrap()
}

#[tokio::test]
#[ignore = "requires bootstrapped local Kine-backed Kubernetes and proxy kubeconfig"]
/// Verify preparation, apply replay, stale reports, restarts, and host-session recovery.
async fn controller_apply_recovery_acceptance() {
    let client = test_client().await;
    let hosts: Api<SuntitleHost> = Api::namespaced(client.clone(), "suntitle");
    let nodes: Api<SuntitleNode> = Api::namespaced(client.clone(), "suntitle");
    let name = format!("controller-test-{}", uuid::Uuid::new_v4());
    hosts
        .create(
            &PostParams::default(),
            &SuntitleHost::new(
                &name,
                SuntitleHostSpec {
                    display_name: name.clone(),
                },
            ),
        )
        .await
        .unwrap();
    let spec = serde_json::from_value(json!({"hostRef":{"name":name},"displayName":"Screen","settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}})).unwrap();
    nodes
        .create(&PostParams::default(), &SuntitleNode::new(&name, spec))
        .await
        .unwrap();
    let (mut controller, mut task) = Controller::start(client.clone(), "suntitle");
    let result = AssertUnwindSafe(async {
        let mut connection = connect(&controller, &name).await;
        assert_ne!(connection.session_id, connection.host.uid);
        assert_eq!(
            hosts
                .get(&name)
                .await
                .unwrap()
                .status
                .unwrap()
                .session_id
                .as_deref(),
            Some(connection.session_id.as_str())
        );
        let first = apply(&controller, &mut connection).await;
        controller
            .report(
                &connection,
                RegistrationClientMessage::Heartbeat {
                    host_uid: connection.host.uid.clone(),
                    session_id: connection.session_id.clone(),
                },
            )
            .await;
        quiet(&mut connection).await;
        for _ in 0..4 {
            quiet(&mut connection).await;
        }
        assert_eq!(
            nodes
                .get(&name)
                .await
                .unwrap()
                .status
                .unwrap()
                .dispatch
                .unwrap()
                .operation_id,
            first.operation_id
        );
        let node = nodes.get(&name).await.unwrap();
        assert!(node.finalizers().iter().any(|f| f == NODE_FINALIZER));
        assert!(node.owner_references().is_empty());
        assert!(
            hosts
                .get(&name)
                .await
                .unwrap()
                .finalizers()
                .iter()
                .any(|f| f == HOST_FINALIZER)
        );
        for stale in [
            ApplyIdentity {
                operation_id: "unknown".into(),
                ..first.clone()
            },
            ApplyIdentity {
                generation: first.generation + 1,
                ..first.clone()
            },
            ApplyIdentity {
                session_id: "old".into(),
                ..first.clone()
            },
            ApplyIdentity {
                target: ResourceIdentity {
                    uid: "old".into(),
                    ..first.target.clone()
                },
                ..first.clone()
            },
        ] {
            ack(&nodes, &controller, &connection, stale, true).await;
            let status = nodes.get(&name).await.unwrap().status.unwrap();
            assert_eq!(status.observed_generation, Some(first.generation));
            assert_eq!(status.dispatch.unwrap().success, None);
            assert!(matches!(status.phase, Some(NodePhase::Pending)));
        }
        ack(&nodes, &controller, &connection, first.clone(), true).await;
        let applied = nodes.get(&name).await.unwrap();
        assert_eq!(
            applied.status.as_ref().unwrap().observed_generation,
            Some(first.generation)
        );
        ack(&nodes, &controller, &connection, first.clone(), true).await;
        assert_eq!(
            applied.resource_version(),
            nodes.get(&name).await.unwrap().resource_version()
        );
        controller
            .report(
                &connection,
                RegistrationClientMessage::Heartbeat {
                    host_uid: connection.host.uid.clone(),
                    session_id: connection.session_id.clone(),
                },
            )
            .await;
        quiet(&mut connection).await;

        nodes
            .patch(
                &name,
                &PatchParams::default(),
                &Patch::Merge(json!({"spec":{"restartToken":"latest-restart"}})),
            )
            .await
            .unwrap();
        let restarted = apply(&controller, &mut connection).await;
        assert_eq!(restarted.restart_token.as_deref(), Some("latest-restart"));
        assert!(restarted.generation > first.generation);
        ack(&nodes, &controller, &connection, first.clone(), true).await;
        assert_eq!(
            nodes
                .get(&name)
                .await
                .unwrap()
                .status
                .unwrap()
                .observed_generation,
            Some(first.generation)
        );
        ack(&nodes, &controller, &connection, restarted.clone(), false).await;
        quiet(&mut connection).await;
        nodes
            .patch(
                &name,
                &PatchParams::default(),
                &Patch::Merge(json!({"spec":{"restartToken":"retry-restart"}})),
            )
            .await
            .unwrap();
        let retry = apply(&controller, &mut connection).await;
        assert_ne!(retry.operation_id, restarted.operation_id);
        ack(&nodes, &controller, &connection, retry.clone(), true).await;
        quiet(&mut connection).await;

        let store = control_api::store::ResourceStore::new(client.clone(), "suntitle");
        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(actix_web::web::Data::new(store))
                .service(control_api::api::nodes::update_node),
        )
        .await;
        let mut disabled = nodes.get(&name).await.unwrap().spec;
        disabled.enabled = false;
        let request = actix_web::test::TestRequest::put()
            .uri(&format!("/nodes/{name}"))
            .set_json(&disabled)
            .to_request();
        let response = actix_web::test::call_service(&app, request).await;
        assert!(response.status().is_success());
        let disabled_identity = apply(&controller, &mut connection).await;
        ack(
            &nodes,
            &controller,
            &connection,
            disabled_identity.clone(),
            true,
        )
        .await;
        assert!(matches!(
            nodes.get(&name).await.unwrap().status.unwrap().phase,
            Some(NodePhase::Disabled)
        ));
        quiet(&mut connection).await;

        let mut replacement = connect(&controller, &name).await;
        assert!(*connection.closed.borrow());
        controller.disconnect(&connection).await;
        assert!(
            !*replacement.closed.borrow(),
            "an old disconnect MUST preserve the replacement socket"
        );
        let resent = apply(&controller, &mut replacement).await;
        assert_eq!(resent.session_id, replacement.session_id);
        assert_ne!(resent.session_id, retry.session_id);
        assert_ne!(resent.operation_id, retry.operation_id);
        assert_eq!(resent.generation, disabled_identity.generation);
        ack(&nodes, &controller, &connection, retry, true).await;
        ack(&nodes, &controller, &replacement, resent, true).await;
        quiet(&mut replacement).await;
        controller.disconnect(&replacement).await;
        assert!(*replacement.closed.borrow());
        controller.shutdown().await;
        (&mut task).await.unwrap();
        let (new_controller, new_task) = Controller::start(client.clone(), "suntitle");
        controller = new_controller;
        task = new_task;
        let mut fresh = connect(&controller, &name).await;
        let identity = apply(&controller, &mut fresh).await;
        ack(&nodes, &controller, &fresh, identity, true).await;
        quiet(&mut fresh).await;

        hosts
            .patch_status(
                &name,
                &PatchParams::default(),
                &Patch::Merge(json!({"status":{"lastHeartbeat":"2000-01-01T00:00:00Z"}})),
            )
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            fresh.closed.wait_for(|closed| *closed),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let host = hosts.get(&name).await.unwrap();
                if matches!(host.status.unwrap().presence, Some(Presence::Offline)) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let mut renewed = connect(&controller, &name).await;
        let identity = apply(&controller, &mut renewed).await;
        let heartbeat = hosts
            .get(&name)
            .await
            .unwrap()
            .status
            .unwrap()
            .last_heartbeat;
        hosts
            .patch_status(
                &name,
                &PatchParams::default(),
                &Patch::Merge(json!({"status":{"sessionId":"externally-replaced"}})),
            )
            .await
            .unwrap();
        controller
            .report(
                &renewed,
                RegistrationClientMessage::Heartbeat {
                    host_uid: renewed.host.uid.clone(),
                    session_id: renewed.session_id.clone(),
                },
            )
            .await;
        ack(&nodes, &controller, &renewed, identity, true).await;
        let status = hosts.get(&name).await.unwrap().status.unwrap();
        assert_eq!(status.session_id.as_deref(), Some("externally-replaced"));
        assert_eq!(
            status.last_heartbeat, heartbeat,
            "a stale connection MUST NOT refresh the persisted heartbeat"
        );
        assert_eq!(
            nodes
                .get(&name)
                .await
                .unwrap()
                .status
                .unwrap()
                .dispatch
                .unwrap()
                .success,
            None,
            "a stale connection MUST NOT acknowledge the pending operation"
        );
    })
    .catch_unwind()
    .await;
    controller.shutdown().await;
    if !task.is_finished() {
        task.await.unwrap();
    }
    // Failed-test cleanup MUST also work after the controller stops.
    nodes
        .patch(
            &name,
            &PatchParams::default(),
            &Patch::Merge(json!({"metadata":{"finalizers":[]}})),
        )
        .await
        .unwrap();
    nodes.delete(&name, &DeleteParams::default()).await.unwrap();
    hosts
        .patch(
            &name,
            &PatchParams::default(),
            &Patch::Merge(json!({"metadata":{"finalizers":[]}})),
        )
        .await
        .unwrap();
    hosts.delete(&name, &DeleteParams::default()).await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Wait for the named removal stage so timeouts identify the lifecycle transition.
async fn remove(connection: &mut Connection, stage: &str) -> ApplyIdentity {
    let message = tokio::time::timeout(Duration::from_secs(15), connection.outbound.recv())
        .await
        .expect(stage)
        .unwrap();
    let RegistrationServerMessage::Remove { identity } = wire(message) else {
        panic!("expected remove")
    };
    identity
}

async fn cleanup_ack(
    nodes: &Api<SuntitleNode>,
    controller: &Controller,
    connection: &Connection,
    identity: ApplyIdentity,
    success: bool,
) {
    controller
        .report(
            connection,
            wire(RegistrationClientMessage::Acknowledgement {
                identity: identity.clone(),
                operation: Operation::Remove,
                success,
                message: None,
            }),
        )
        .await;
    if success {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let Some(node) = nodes.get_opt(&identity.target.name).await.unwrap() else {
                    break;
                };
                if !node.finalizers().iter().any(|f| f == NODE_FINALIZER) {
                    break;
                }
                let dispatch = node.status.as_ref().unwrap().dispatch.as_ref().unwrap();
                if dispatch.operation_id != identity.operation_id || dispatch.success != Some(true)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires bootstrapped local Kine-backed Kubernetes and proxy kubeconfig"]
/// Verify artifact-backed node deletion, reconnect retries, and deferred offline host cleanup.
async fn controller_deletion_acceptance() {
    let client = test_client().await;
    let hosts: Api<SuntitleHost> = Api::namespaced(client.clone(), "suntitle");
    let nodes: Api<SuntitleNode> = Api::namespaced(client.clone(), "suntitle");
    let name = format!("cleanup-test-{}", uuid::Uuid::new_v4());
    let node_names = [format!("{name}-online"), format!("{name}-offline")];
    let (mut controller, mut task) = Controller::start(client.clone(), "suntitle");
    let result = AssertUnwindSafe(async {
        let mut connection = connect(&controller, &name).await;
        let spec: SuntitleNodeSpec = serde_json::from_value(json!({"hostRef":{"name":name},"displayName":"Screen","settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}})).unwrap();
        nodes.create(&PostParams::default(), &SuntitleNode::new(&node_names[0], spec.clone())).await.unwrap();
        let applied = apply(&controller, &mut connection).await;
        ack(&nodes, &controller, &connection, applied.clone(), true).await;
        let store = control_api::store::ResourceStore::new(client.clone(), "suntitle");
        let app = actix_web::test::init_service(actix_web::App::new()
            .app_data(actix_web::web::Data::new(store))
            .service(control_api::api::nodes::delete_node)).await;
        nodes.patch(&node_names[0], &PatchParams::default(), &Patch::Merge(json!({"metadata":{"finalizers":[NODE_FINALIZER,"test.suntitle.io/hold"]}}))).await.unwrap();
        let response = actix_web::test::call_service(&app, actix_web::test::TestRequest::delete()
            .uri(&format!("/nodes/{}", node_names[0])).to_request()).await;
        assert_eq!(response.status(), actix_web::http::StatusCode::ACCEPTED);
        let first = remove(&mut connection, "initial node removal").await;
        assert!(nodes.get(&node_names[0]).await.unwrap().status.unwrap().dispatch.unwrap().artifact_reference.is_none());
        quiet(&mut connection).await;
        ack(&nodes, &controller, &connection, first.clone(), true).await;
        cleanup_ack(&nodes, &controller, &connection, ApplyIdentity { operation_id: "wrong".into(), ..first.clone() }, true).await;
        assert!(nodes.get(&node_names[0]).await.unwrap().finalizers().iter().any(|f| f == NODE_FINALIZER));
        cleanup_ack(&nodes, &controller, &connection, first.clone(), false).await;
        assert!(nodes.get_opt(&node_names[0]).await.unwrap().is_some());
        quiet(&mut connection).await;
        connection = connect(&controller, &name).await;
        let retry = remove(&mut connection, "removal after reconnect").await;
        assert_ne!(retry.operation_id, first.operation_id);
        cleanup_ack(&nodes, &controller, &connection, retry.clone(), true).await;
        cleanup_ack(&nodes, &controller, &connection, retry, true).await;
        assert_eq!(nodes.get(&node_names[0]).await.unwrap().finalizers(), &["test.suntitle.io/hold"]);
        nodes.patch(&node_names[0], &PatchParams::default(), &Patch::Merge(json!({"metadata":{"finalizers":[]}}))).await.unwrap();
        assert!(nodes.get_opt(&node_names[0]).await.unwrap().is_none());

        nodes.create(&PostParams::default(), &SuntitleNode::new(&node_names[1], spec)).await.unwrap();
        let applied = apply(&controller, &mut connection).await;
        ack(&nodes, &controller, &connection, applied, true).await;
        controller.disconnect(&connection).await;
        hosts.delete(&name, &DeleteParams::default()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if nodes.get(&node_names[1]).await.unwrap().metadata.deletion_timestamp.is_some() { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        assert!(hosts.get(&name).await.unwrap().finalizers().iter().any(|f| f == HOST_FINALIZER));
        assert!(nodes.get(&node_names[1]).await.unwrap().finalizers().iter().any(|f| f == NODE_FINALIZER));
        controller.shutdown().await;
        (&mut task).await.unwrap();
        let (replacement, replacement_task) = Controller::start(client.clone(), "suntitle");
        controller = replacement;
        task = replacement_task;
        let mut cleanup = connect(&controller, &name).await;
        let removal = remove(&mut cleanup, "offline host cleanup after reconnect").await;
        assert_eq!(removal.host_uid, connection.host.uid);
        cleanup_ack(&nodes, &controller, &cleanup, removal, true).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if hosts.get_opt(&name).await.unwrap().is_none() { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        assert!(nodes.get_opt(&node_names[1]).await.unwrap().is_none());
        assert!(*cleanup.closed.borrow());
        let recreated = connect(&controller, &name).await;
        assert_ne!(recreated.host.uid, connection.host.uid);
    }).catch_unwind().await;
    controller.shutdown().await;
    if !task.is_finished() {
        task.await.unwrap();
    }
    for node_name in &node_names {
        if nodes.get_opt(node_name).await.unwrap().is_some() {
            nodes
                .patch(
                    node_name,
                    &PatchParams::default(),
                    &Patch::Merge(json!({"metadata":{"finalizers":[]}})),
                )
                .await
                .unwrap();
            if nodes.get_opt(node_name).await.unwrap().is_some() {
                nodes
                    .delete(node_name, &DeleteParams::default())
                    .await
                    .unwrap();
            }
        }
    }
    if hosts.get_opt(&name).await.unwrap().is_some() {
        hosts
            .patch(
                &name,
                &PatchParams::default(),
                &Patch::Merge(json!({"metadata":{"finalizers":[]}})),
            )
            .await
            .unwrap();
        if hosts.get_opt(&name).await.unwrap().is_some() {
            hosts.delete(&name, &DeleteParams::default()).await.unwrap();
        }
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
