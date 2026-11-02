//! Publication tests exercise immutable task identity and status ordering independently of Kubernetes watches.
use super::*;
use serde_json::json;

/// Build a local-source resource with immutable Kubernetes identity.
fn extension() -> SuntitleExtension {
    serde_json::from_value(json!({"metadata":{"name":"fixture","namespace":"test","uid":"uid-1","resourceVersion":"1","generation":1},"spec":{"name":"Fixture","sourceType":"local","source":"/missing-extension","adapter":{"type":"transcriber","execution":{"runtime":"executable","command":"./run"}}}})).unwrap()
}
/// Use anonymous loopback publication without credentials or implicit TLS exceptions.
fn config() -> PublicationConfig {
    PublicationConfig {
        repository: "127.0.0.1:1/extensions".into(),
        authentication: RegistryAuth::Anonymous,
        source_authentication: Default::default(),
        transport: TransportOptions {
            plain_http: true,
            ..Default::default()
        },
        concurrency: 2,
        task_timeout: Duration::from_secs(5),
    }
}

/// Spec, refresh, destination, and recreated resource identities MUST invalidate old tasks.
#[test]
fn execution_identity_tracks_desired_inputs_without_reading_source() {
    let mut resource = extension();
    let mut options = config();
    let original = ExecutionId::for_extension(&resource, &options).unwrap();
    assert_eq!(
        original,
        ExecutionId::for_extension(&resource, &options).unwrap()
    );
    assert_eq!(
        ExecutionId::parse(&original.encode()),
        Some(original.clone())
    );
    let retry = original.next_attempt();
    assert!(retry.matches(&original));
    assert_eq!(retry.attempt(), 1);
    assert_ne!(retry.encode(), original.encode());
    assert!(ExecutionId::parse("malformed").is_none());
    resource.spec.refresh_token = Some("refresh".into());
    assert_ne!(
        original,
        ExecutionId::for_extension(&resource, &options).unwrap()
    );
    resource = extension();
    resource.metadata.uid = Some("uid-2".into());
    assert_ne!(
        original,
        ExecutionId::for_extension(&resource, &options).unwrap()
    );
    resource = extension();
    options.repository.push_str("/other");
    assert_ne!(
        original,
        ExecutionId::for_extension(&resource, &options).unwrap()
    );
}

/// Duplicate acceptance preserves results, and notification follows terminal storage.
#[tokio::test]
async fn duplicate_attempts_notify_after_failure_and_shutdown_cleanly() {
    let cancellation = CancellationToken::new();
    let sources = sources::Sources::new(config(), cancellation.clone()).unwrap();
    let (notify, mut notifications) = mpsc::unbounded_channel();
    let (executor, worker) = Executor::start(sources, 2, cancellation.clone(), notify);
    let resource = extension();
    let task = PublishExtension {
        id: ExecutionId::for_extension(&resource, &config()).unwrap(),
        key: ObjectRef::from_obj(&resource),
        spec: resource.spec.clone(),
        destination: "127.0.0.1:1/extensions/fixture:attempt-0".into(),
    };
    let (first, duplicate) = tokio::join!(
        executor.enqueue(task.clone()),
        executor.enqueue(task.clone())
    );
    assert_eq!(first.unwrap(), duplicate.unwrap());
    let key = tokio::time::timeout(Duration::from_secs(5), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(key, task.key);
    let terminal = executor.status(&task.id).await.unwrap();
    assert!(matches!(terminal, Some(TaskStatus::Failed { .. })));
    executor.enqueue(task.clone()).await.unwrap();
    assert_eq!(executor.status(&task.id).await.unwrap(), terminal);
    assert!(notifications.try_recv().is_err());
    assert!(
        executor
            .status(&ExecutionId::parse(&format!("{}-0", "f".repeat(64))).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
}

/// Status CAS prevents an obsolete worker from marking a newer revision ready.
#[tokio::test]
async fn status_writes_include_uid_and_resource_version_and_preserve_other_conditions() {
    use warp::Filter;
    let resource = extension();
    let resource_reply = resource.clone();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let patch = warp::patch()
        .and(warp::body::bytes().map(|body: actix_web::web::Bytes| {
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        }))
        .map(move |body: serde_json::Value| {
            tx.send(body).unwrap();
            warp::reply::json(&resource_reply)
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(warp::serve(patch).incoming(listener).run());
    let client = kube::Client::try_from(kube::Config::new(
        format!("http://{address}").parse().unwrap(),
    ))
    .unwrap();
    let api = Api::namespaced(client, "test");
    write_status(
        &api,
        &resource,
        PublicationOutcome::Published {
            task: ExecutionId::for_extension(&resource, &config()).unwrap(),
            reference: "registry/extension@sha256:abc".into(),
        },
    )
    .await
    .unwrap();
    let patch = rx.recv().await.unwrap();
    assert_eq!(patch["metadata"]["uid"], "uid-1");
    assert_eq!(patch["metadata"]["resourceVersion"], "1");
    assert_eq!(patch["status"]["observedGeneration"], 1);
    assert_eq!(patch["status"]["conditions"][0]["status"], "True");
    write_status(
        &api,
        &resource,
        PublicationOutcome::Preparing {
            task: ExecutionId::for_extension(&resource, &config())
                .unwrap()
                .next_attempt(),
        },
    )
    .await
    .unwrap();
    assert!(rx.recv().await.unwrap()["status"]["artifactReference"].is_null());
    server.abort();
}
