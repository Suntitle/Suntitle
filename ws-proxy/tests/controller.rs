use futures_util::FutureExt;
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, Patch, PatchParams, PostParams},
};
use serde_json::json;
use std::{collections::BTreeMap, panic::AssertUnwindSafe, path::Path, time::Duration};
use ws_proxy::{
    controller::{Connection, Controller},
    kubernetes::{client_from_kubeconfig, resources::*},
    registration::*,
};

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
async fn apply(connection: &mut Connection) -> ApplyIdentity {
    let message = tokio::time::timeout(Duration::from_secs(15), connection.outbound.recv())
        .await
        .unwrap()
        .unwrap();
    let RegistrationServerMessage::Apply { identity, .. } = wire(message) else {
        panic!("expected apply")
    };
    identity
}
async fn ack(
    controller: &Controller,
    connection: &Connection,
    identity: ApplyIdentity,
    success: bool,
) {
    controller
        .report(
            &connection.host.uid,
            &connection.session_id,
            wire(RegistrationClientMessage::Acknowledgement {
                identity,
                operation: Operation::Apply,
                success,
                message: None,
            }),
        )
        .await;
}
async fn quiet(connection: &mut Connection) {
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), connection.outbound.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires bootstrapped local Kine-backed Kubernetes and proxy kubeconfig"]
async fn controller_apply_recovery_acceptance() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let client = client_from_kubeconfig(&root.join("pki/suntitle-proxy.kubeconfig"))
        .await
        .unwrap();
    let hosts: Api<SuntitleHost> = Api::namespaced(client.clone(), "suntitle");
    let nodes: Api<SuntitleNode> = Api::namespaced(client.clone(), "suntitle");
    let name = format!("controller-test-{}", uuid::Uuid::new_v4());
    hosts
        .create(
            &PostParams::default(),
            &SuntitleHost::new(&name, SuntitleHostSpec { name: name.clone() }),
        )
        .await
        .unwrap();
    let spec = serde_json::from_value(json!({"hostRef":{"name":name},"name":"Screen","settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}})).unwrap();
    nodes
        .create(&PostParams::default(), &SuntitleNode::new(&name, spec))
        .await
        .unwrap();
    let (mut controller, mut task) = Controller::start(client.clone(), "suntitle");
    let result = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(15), controller.synchronized())
            .await
            .unwrap()
            .unwrap();
        let mut connection = connect(&controller, &name).await;
        let first = apply(&mut connection).await;
        let retry = apply(&mut connection).await;
        assert_eq!(
            first, retry,
            "lost acknowledgements MUST retain operation identity"
        );
        let node = nodes.get(&name).await.unwrap();
        assert!(node.finalizers().iter().any(|f| f == NODE_FINALIZER));
        assert_eq!(node.owner_references()[0].uid, connection.host.uid);
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
            ack(&controller, &connection, stale, true).await;
            assert!(
                nodes
                    .get(&name)
                    .await
                    .unwrap()
                    .status
                    .unwrap()
                    .observed_generation
                    .is_none()
            );
        }
        ack(&controller, &connection, first.clone(), true).await;
        let applied = nodes.get(&name).await.unwrap();
        assert_eq!(
            applied.status.as_ref().unwrap().observed_generation,
            Some(first.generation)
        );
        ack(&controller, &connection, first.clone(), true).await;
        assert_eq!(
            applied.resource_version(),
            nodes.get(&name).await.unwrap().resource_version()
        );
        controller
            .report(
                &connection.host.uid,
                &connection.session_id,
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
        let restarted = apply(&mut connection).await;
        assert_eq!(restarted.restart_token.as_deref(), Some("latest-restart"));
        assert!(restarted.generation > first.generation);
        ack(&controller, &connection, first.clone(), true).await;
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
        ack(&controller, &connection, restarted.clone(), false).await;
        let retry = apply(&mut connection).await;
        assert_ne!(retry.operation_id, restarted.operation_id);
        ack(&controller, &connection, retry.clone(), true).await;
        quiet(&mut connection).await;

        let store = ws_proxy::kubernetes::store::ResourceStore::new(client.clone(), "suntitle");
        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(actix_web::web::Data::new(store))
                .service(ws_proxy::api::nodes::update_node),
        )
        .await;
        let mut disabled = nodes.get(&name).await.unwrap().spec;
        disabled.enabled = false;
        let request = actix_web::test::TestRequest::put()
            .uri(&format!("/nodes/{name}"))
            .set_json(json!({"spec": disabled}))
            .to_request();
        let response = actix_web::test::call_service(&app, request).await;
        assert!(response.status().is_success());
        let disabled_identity = apply(&mut connection).await;
        ack(&controller, &connection, disabled_identity.clone(), true).await;
        assert!(matches!(
            nodes.get(&name).await.unwrap().status.unwrap().phase,
            Some(NodePhase::Disabled)
        ));
        quiet(&mut connection).await;

        let mut replacement = connect(&controller, &name).await;
        assert!(*connection.closed.borrow());
        controller
            .disconnect(&connection.host.uid, &connection.session_id)
            .await;
        assert!(
            !*replacement.closed.borrow(),
            "an old disconnect MUST preserve the replacement socket"
        );
        let resent = apply(&mut replacement).await;
        assert_ne!(resent.session_id, retry.session_id);
        assert_eq!(resent.generation, disabled_identity.generation);
        ack(&controller, &connection, retry, true).await;
        ack(&controller, &replacement, resent, true).await;
        quiet(&mut replacement).await;
        controller
            .disconnect(&replacement.host.uid, &replacement.session_id)
            .await;
        assert!(*replacement.closed.borrow());
        controller.shutdown().await;
        (&mut task).await.unwrap();
        let (new_controller, new_task) = Controller::start(client.clone(), "suntitle");
        controller = new_controller;
        task = new_task;
        controller.synchronized().await.unwrap();
        let mut fresh = connect(&controller, &name).await;
        let identity = apply(&mut fresh).await;
        ack(&controller, &fresh, identity, true).await;
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
        let identity = apply(&mut renewed).await;
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
                &renewed.host.uid,
                &renewed.session_id,
                RegistrationClientMessage::Heartbeat {
                    host_uid: renewed.host.uid.clone(),
                    session_id: renewed.session_id.clone(),
                },
            )
            .await;
        ack(&controller, &renewed, identity, true).await;
        let status = hosts.get(&name).await.unwrap().status.unwrap();
        assert_eq!(status.session_id.as_deref(), Some("externally-replaced"));
        assert_eq!(
            status.last_heartbeat, heartbeat,
            "a stale connection MUST NOT refresh the persisted heartbeat"
        );
        assert_ne!(
            nodes
                .get(&name)
                .await
                .unwrap()
                .status
                .unwrap()
                .reporting_session
                .as_deref(),
            Some(renewed.session_id.as_str())
        );
    })
    .catch_unwind()
    .await;
    controller.shutdown().await;
    if !task.is_finished() {
        task.await.unwrap();
    }
    // Test resources MUST be removed explicitly; deletion cleanup belongs to phase 4.
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
