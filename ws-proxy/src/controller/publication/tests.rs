//! Publication tests exercise immutable task identity and status ordering independently of Kubernetes watches.
use super::*;
use serde_json::json;

/// Build a local-source resource with immutable Kubernetes identity.
pub(super) fn extension() -> SuntitleExtension {
    serde_json::from_value(json!({"metadata":{"name":"fixture","namespace":"test","uid":"uid-1","resourceVersion":"1","generation":1},"spec":{"displayName":"Fixture","sourceType":"local","source":"/missing-extension","adapter":{"inputType":"text","outputType":"text","execution":{"runtime":"executable","command":"./run"}}}})).unwrap()
}
/// Use anonymous loopback publication without credentials or implicit TLS exceptions.
pub(super) fn config() -> PublicationConfig {
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
    let original = identity::task_id(&resource, &options).unwrap();
    assert_eq!(resource.spec.max_attempts, 3);
    assert_eq!(original, identity::task_id(&resource, &options).unwrap());
    resource.spec.refresh_token = Some("refresh".into());
    assert_ne!(original, identity::task_id(&resource, &options).unwrap());
    resource.spec.manifest = Some("displayName: Changed".into());
    assert_ne!(original, identity::task_id(&resource, &options).unwrap());
    resource = extension();
    resource.metadata.uid = Some("uid-2".into());
    assert_ne!(original, identity::task_id(&resource, &options).unwrap());
    resource = extension();
    options.repository.push_str("/other");
    assert_ne!(original, identity::task_id(&resource, &options).unwrap());
}

/// Submit one immutable command through native Apalis storage.
async fn push(
    storage: &SqliteStorage<PublishExtension>,
    resource: &SuntitleExtension,
    options: &PublicationConfig,
    id: &TaskId,
) {
    storage
        .clone()
        .push_task(
            TaskBuilder::new(PublishExtension {
                destination: format!(
                    "{}/{}:publication-{id}",
                    options.repository,
                    resource.name_any()
                ),
                namespace: resource.namespace(),
                name: resource.name_any(),
                spec: resource.spec.clone(),
            })
            .task_id(id.clone())
            .idempotency_key(id.to_string())
            .max_attempts(resource.spec.max_attempts as usize)
            .build(),
        )
        .await
        .unwrap();
}

/// Read only persisted terminal results; pending and running tasks return no result.
async fn result(
    storage: &SqliteStorage<PublishExtension>,
    id: &TaskId,
) -> Option<Result<executor::PublicationResult, String>> {
    storage
        .clone()
        .check_status([id.clone()])
        .await
        .unwrap()
        .pop()
        .map(|task: TaskResult<executor::PublicationResult>| task.result)
}

/// SQLite deduplicates concurrent and completed submissions; notification follows terminal storage.
#[tokio::test]
async fn duplicate_attempts_notify_after_failure_and_shutdown_cleanly() {
    let cancellation = CancellationToken::new();
    let options = config();
    let sources = Sources::new(options.clone(), cancellation.clone()).unwrap();
    let storage = storage().await.unwrap();
    let (notify, mut notifications) = mpsc::unbounded_channel();
    let worker = executor::start(storage.clone(), sources, 2, cancellation.clone(), notify);
    let mut resource = extension();
    resource.spec.adapter = None;
    let id = identity::task_id(&resource, &options).unwrap();
    tokio::join!(
        push(&storage, &resource, &options, &id),
        push(&storage, &resource, &options, &id)
    );
    let key = tokio::time::timeout(Duration::from_secs(5), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(key, ObjectRef::from_obj(&resource));
    let terminal = result(&storage, &id).await;
    assert!(matches!(&terminal, Some(Err(error)) if error.contains("absolute directory")));
    push(&storage, &resource, &options, &id).await;
    assert!(matches!(result(&storage, &id).await, Some(Err(_))));
    assert!(notifications.try_recv().is_err());
    let stored = storage.clone().fetch_by_id(&id).await.unwrap().unwrap();
    assert_eq!(stored.ctx().attempt(), 3);
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
        &identity::task_id(&resource, &config()).unwrap(),
        &Some(TaskResult {
            task_id: identity::task_id(&resource, &config()).unwrap(),
            attempt: 1,
            status: Status::Done,
            result: Ok(executor::PublicationResult { reference: "registry/extension@sha256:abc".into(), spec: resource.spec.clone() }),
        }),
    )
    .await
    .unwrap();
    let patch = rx.recv().await.unwrap();
    assert_eq!(patch["metadata"]["uid"], "uid-1");
    assert_eq!(patch["metadata"]["resourceVersion"], "1");
    assert_eq!(patch["status"]["observedGeneration"], 1);
    assert_eq!(patch["status"]["attempts"], 1);
    assert_eq!(patch["status"]["resolvedSpec"]["adapter"]["execution"]["command"], "./run");
    assert_eq!(patch["status"]["conditions"][0]["status"], "True");
    write_status(
        &api,
        &resource,
        &identity::task_id(&resource, &config()).unwrap(),
        &None,
    )
    .await
    .unwrap();
    assert!(rx.recv().await.unwrap()["status"]["artifactReference"].is_null());
    server.abort();
}

/// Conflicts defer guarded writes; infrastructure failures MUST remain observable.
#[tokio::test]
async fn status_conflicts_requeue_without_retrying_stale_versions() {
    use warp::Filter;
    for code in [409, 403, 500] {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let route = warp::patch()
            .and(warp::body::bytes().map(|body: actix_web::web::Bytes| {
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }))
            .map(move |body: serde_json::Value| {
                tx.send(body).unwrap();
                warp::reply::with_status(
                    warp::reply::json(&serde_json::json!({
                        "apiVersion": "v1", "kind": "Status", "status": "Failure",
                        "code": code, "reason": "Conflict", "message": "fixture rejection"
                    })),
                    warp::http::StatusCode::from_u16(code).unwrap(),
                )
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        )).unwrap();
        let api = Api::namespaced(client, "test");
        let resource = extension();
        let action = write_status(
            &api, &resource, &identity::task_id(&resource, &config()).unwrap(), &None,
        ).await;
        if code == 409 {
            assert_eq!(action.unwrap(), Action::requeue(Duration::from_secs(1)));
        } else {
            assert!(action.is_err());
        }
        let patch = rx.recv().await.unwrap();
        assert_eq!(patch["metadata"]["uid"], "uid-1");
        assert_eq!(patch["metadata"]["resourceVersion"], "1");
        assert!(rx.try_recv().is_err(), "MUST NOT retry the obsolete snapshot");
        server.abort();
    }
}

/// A blocked transfer MUST NOT block native storage calls or exceed worker concurrency.
/// Shutdown joins active cancellation; queued work is reconstructed after process restart.
#[tokio::test]
async fn slow_publication_keeps_the_command_api_responsive_and_bounds_concurrency() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use warp::Filter;
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let handler_started = started.clone();
    let handler_release = release.clone();
    let route = warp::any().and_then(move || {
        let started = handler_started.clone();
        let release = handler_release.clone();
        async move {
            started.fetch_add(1, Ordering::SeqCst);
            release.acquire().await.unwrap().forget();
            Ok::<_, std::convert::Infallible>(warp::reply::with_status(
                "fixture failure",
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            ))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(warp::serve(route).incoming(listener).run());
    let cancellation = CancellationToken::new();
    let mut options = config();
    options.task_timeout = Duration::from_secs(30);
    let sources = Sources::new(options.clone(), cancellation.clone()).unwrap();
    let storage = storage().await.unwrap();
    let (notify, mut notifications) = mpsc::unbounded_channel();
    let worker = executor::start(storage.clone(), sources, 1, cancellation.clone(), notify);
    let mut resource = extension();
    resource.spec.max_attempts = 1;
    resource.spec.source_type = suntitle_core::kubernetes::resources::ExtensionSourceType::Http;
    resource.spec.source = format!("http://{address}/fixture");
    let first_resource = resource.clone();
    let first = identity::task_id(&resource, &options).unwrap();
    push(&storage, &resource, &options, &first).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while started.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        push(&storage, &first_resource, &options, &first),
    )
    .await
    .unwrap();
    let running = tokio::time::timeout(Duration::from_secs(1), storage.clone().fetch_by_id(&first))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(running.status(), Status::Running);
    resource.metadata.name = Some("second-resource".into());
    resource.metadata.uid = Some("uid-2".into());
    let second = identity::task_id(&resource, &options).unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        push(&storage, &resource, &options, &second),
    )
    .await
    .unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert!(notifications.try_recv().is_err());
    release.add_permits(1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), notifications.recv())
            .await
            .unwrap(),
        Some(ObjectRef::from_obj(&first_resource))
    );
    let terminal = result(&storage, &first).await;
    assert!(matches!(&terminal, Some(Err(_))));
    push(&storage, &first_resource, &options, &first).await;
    assert_eq!(serde_json::to_value(result(&storage, &first).await).unwrap(), serde_json::to_value(terminal).unwrap());
    tokio::time::timeout(Duration::from_secs(5), async {
        while started.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut third_resource = resource.clone();
    third_resource.metadata.uid = Some("uid-3".into());
    let third = identity::task_id(&third_resource, &options).unwrap();
    push(&storage, &third_resource, &options, &third).await;
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 2);
    assert!(matches!(result(&storage, &second).await, Some(Err(_))));
    assert!(result(&storage, &third).await.is_none());
    server.abort();
}

/// Losing the notification consumer MUST stop workers and cancel active transfers.
#[tokio::test]
async fn closed_notification_receiver_stops_worker() {
    let cancellation = CancellationToken::new();
    let sources = Sources::new(config(), cancellation.clone()).unwrap();
    let storage = storage().await.unwrap();
    let (notify, notifications) = mpsc::unbounded_channel();
    let worker = executor::start(storage, sources, 1, cancellation.clone(), notify);
    drop(notifications);
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(cancellation.is_cancelled());
}
