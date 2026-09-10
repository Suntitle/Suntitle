use super::session::{current_session, heartbeat_expired, identity, matches_node};
use super::*;
use crate::{kubernetes::resources::*, registration::*};
use chrono::Utc;
use serde_json::{Value, json};

fn state() -> State {
    let client =
        Client::try_from(kube::Config::new("http://127.0.0.1:9".parse().unwrap())).unwrap();
    State {
        hosts_api: Api::namespaced(client.clone(), "test"),
        nodes_api: Api::namespaced(client, "test"),
        connections: Connections::default(),
    }
}

pub(super) fn node() -> SuntitleNode {
    serde_json::from_value(json!({"apiVersion":"control.suntitle.io/v1alpha1","kind":"SuntitleNode","metadata":{"namespace":"test","name":"screen","uid":"node-uid","generation":1,"resourceVersion":"opaque"},"spec":{"hostRef":{"name":"host"},"name":"Screen","settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}}})).unwrap()
}

#[tokio::test]
async fn expiry_persists_offline_status_once_for_resource_watches() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use warp::Filter;

    let mut host = SuntitleHost::new(
        "host",
        SuntitleHostSpec {
            name: "Host".into(),
        },
    );
    host.metadata.namespace = Some("test".into());
    host.metadata.uid = Some("host-uid".into());
    host.metadata.resource_version = Some("1".into());
    let status = host.status.get_or_insert_default();
    status.presence = Some(Presence::Online);
    status.session_id = Some("session".into());
    status.last_heartbeat = Some(Utc::now() - chrono::Duration::seconds(46));
    let resource = Arc::new(std::sync::Mutex::new(host));
    let listed = resource.clone();
    let list = warp::get().map(move || {
        warp::reply::json(&json!({
            "apiVersion": "control.suntitle.io/v1alpha1",
            "kind": "SuntitleHostList",
            "metadata": {"resourceVersion": "1"},
            "items": [*listed.lock().unwrap()]
        }))
    });
    let patches = Arc::new(AtomicUsize::new(0));
    let written = resource.clone();
    let count = patches.clone();
    let patch = warp::patch()
        .and(
            warp::body::bytes()
                .map(|body: actix_web::web::Bytes| serde_json::from_slice::<Value>(&body).unwrap()),
        )
        .map(move |body: Value| {
            count.fetch_add(1, Ordering::SeqCst);
            let mut host = written.lock().unwrap();
            host.status = serde_json::from_value(body["status"].clone()).unwrap();
            warp::reply::json(&*host)
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(warp::serve(list.or(patch)).incoming(listener).run());
    let mut state = state();
    let client = Client::try_from(kube::Config::new(
        format!("http://{address}").parse().unwrap(),
    ))
    .unwrap();
    state.hosts_api = Api::namespaced(client, "test");
    let connection = state.connections.open(
        identity(&resource.lock().unwrap().metadata),
        "session".into(),
    );
    let host = resource.lock().unwrap().clone();
    state.offline(&host).await.unwrap();
    assert!(*connection.closed.borrow());
    assert!(matches!(
        resource.lock().unwrap().status.as_ref().unwrap().presence,
        Some(Presence::Offline)
    ));
    assert_eq!(patches.load(Ordering::SeqCst), 1);
    let host = resource.lock().unwrap().clone();
    state.offline(&host).await.unwrap();
    assert_eq!(patches.load(Ordering::SeqCst), 1);
    server.abort();
}

#[test]
fn heartbeat_expiry_and_session_identity_use_resource_status() {
    let mut host = SuntitleHost::new(
        "host",
        SuntitleHostSpec {
            name: "Host".into(),
        },
    );
    host.metadata.uid = Some("uid".into());
    let now = Utc::now();
    assert!(heartbeat_expired(&host, now));
    let status = host.status.get_or_insert_default();
    status.last_heartbeat = Some(now);
    status.session_id = Some("current".into());
    assert!(!heartbeat_expired(&host, now));
    assert!(heartbeat_expired(
        &host,
        now + chrono::Duration::seconds(45)
    ));
    assert!(current_session(&host, "uid", "current"));
    assert!(!current_session(&host, "uid", "old"));
    assert!(!current_session(&host, "old-uid", "current"));
}

#[tokio::test]
async fn outbound_overflow_closes_session() {
    let mut state = state();
    let connection = state.connections.open(
        ResourceIdentity {
            namespace: "test".into(),
            name: "host".into(),
            uid: "host-uid".into(),
        },
        "session".into(),
    );
    for _ in 0..32 {
        state.dispatch(
            "host-uid",
            "session",
            RegistrationServerMessage::Error {
                message: "queued".into(),
            },
        );
    }
    assert!(!*connection.closed.borrow());
    state.dispatch(
        "host-uid",
        "session",
        RegistrationServerMessage::Error {
            message: "overflow".into(),
        },
    );
    assert!(*connection.closed.borrow());
    assert!(state.connections.hosts().is_empty());
}

#[test]
fn stale_incarnation_generation_token_and_deletion_reject_reports() {
    let mut node = node();
    let mut id = ApplyIdentity {
        target: identity(&node.metadata),
        host_uid: "host".into(),
        session_id: "session".into(),
        generation: 1,
        restart_token: None,
        operation_id: "op".into(),
    };
    assert!(matches_node(&node, &id));
    id.target.uid = "old".into();
    assert!(!matches_node(&node, &id));
    id.target.uid = "node-uid".into();
    id.generation = 2;
    assert!(!matches_node(&node, &id));
    id.generation = 1;
    id.restart_token = Some("old".into());
    assert!(!matches_node(&node, &id));
    id.restart_token = None;
    node.metadata.deletion_timestamp = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::now(),
    ));
    assert!(!matches_node(&node, &id));
}
