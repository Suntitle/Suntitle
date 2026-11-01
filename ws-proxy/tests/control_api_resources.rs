use actix_web::{
    App,
    http::{Method, StatusCode},
    test,
};
use futures_util::FutureExt;
use kube::{
    Api, Client, ResourceExt,
    api::{DeleteParams, Patch, PatchParams, PostParams},
};
use serde_json::{Value, json};
use std::{panic::AssertUnwindSafe, path::Path};
use utoipa_actix_web::AppExt;
use uuid::Uuid;
use ws_proxy::kubernetes::{client_from_kubeconfig, resources::*, store::ResourceStore};

// Each request MUST use fresh API state to exercise persistence across restarts.
async fn request(
    store: &ResourceStore,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value, Option<String>) {
    let app = test::init_service(
        App::new()
            .into_utoipa_app()
            .configure(|c| ws_proxy::api::configure_with_store(c, store.clone()))
            .into_app(),
    )
    .await;
    let mut req = test::TestRequest::default().method(method).uri(uri);
    if let Some(body) = body {
        req = req.set_json(body);
    }
    let response = test::call_service(&app, req.to_request()).await;
    let status = response.status();
    let location = response
        .headers()
        .get("Location")
        .map(|h| h.to_str().unwrap().to_owned());
    let bytes = test::read_body(response).await;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value, location)
}

async fn expect(
    store: &ResourceStore,
    method: Method,
    uri: &str,
    body: Option<Value>,
    status: u16,
) -> Value {
    let (actual, value, _) = request(store, method, uri, body).await;
    assert_eq!(actual.as_u16(), status, "{uri}: {value}");
    if status >= 400 {
        assert!(value["error"]["code"].is_string(), "{value}");
    }
    value
}

#[actix_web::test]
async fn unavailable_and_malformed_requests_have_safe_errors() {
    let client =
        Client::try_from(kube::Config::new("http://127.0.0.1:9".parse().unwrap())).unwrap();
    let store = ResourceStore::new(client, "suntitle");
    for uri in [
        "/api/v1/nodes",
        "/api/v1/hosts",
        "/api/v1/extensions",
        "/api/v1/extensions/example",
        "/api/v1/nodes/example",
        "/readyz",
    ] {
        let value = expect(&store, Method::GET, uri, None, 503).await;
        assert_eq!(value["error"]["code"], "Unavailable");
        assert!(!value.to_string().contains("127.0.0.1"));
    }
    let value = expect(
        &store,
        Method::PUT,
        "/api/v1/nodes/example",
        Some(json!({"spec":{}})),
        400,
    )
    .await;
    assert_eq!(value["error"]["code"], "InvalidRequest");
}

#[actix_web::test]
#[ignore = "requires bootstrapped local Kine-backed Kubernetes and certificate kubeconfigs"]
async fn resource_api_acceptance() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let api_client = client_from_kubeconfig(&root.join("pki/suntitle-api.kubeconfig"))
        .await
        .unwrap();
    let proxy_client = client_from_kubeconfig(&root.join("pki/suntitle-proxy.kubeconfig"))
        .await
        .unwrap();
    let store = ResourceStore::new(api_client, "suntitle");
    let hosts: Api<SuntitleHost> = Api::namespaced(proxy_client.clone(), "suntitle");
    let nodes: Api<SuntitleNode> = Api::namespaced(proxy_client, "suntitle");
    let prefix = format!("phase2-{}", Uuid::new_v4().simple());
    let host_name = format!("{prefix}-host");
    let host = hosts
        .create(
            &PostParams::default(),
            &SuntitleHost::new(
                &host_name,
                SuntitleHostSpec {
                    name: "Phase 2 fixture".into(),
                },
            ),
        )
        .await
        .unwrap();
    let host_uid = host.uid().unwrap();
    let mut created = Vec::<String>::new();
    let result = AssertUnwindSafe(async {
        expect(&store, Method::GET, "/readyz", None, 200).await;
        let variants: Value = serde_json::from_str(include_str!("fixtures/kubernetes-node-specs.json")).unwrap();
        for kind in ["audio_source", "transcriber", "translator", "display"] {
            let name = format!("{prefix}-{}", kind.replace('_', "-"));
            let spec = json!({"name":kind,"hostRef":{"name":"ignored-host"},"settings":{kind:variants[kind]},"inputRef":{"name":"missing-input"}});
            let (status, node, location) = request(&store, Method::POST, &format!("/api/v1/hosts/{host_uid}/nodes"), Some(json!({"name":name,"spec":spec}))).await;
            assert_eq!(status, StatusCode::CREATED, "{node}");
            created.push(name.clone());
            assert_eq!(location, Some(format!("/api/v1/nodes/{name}")));
            assert_eq!(node["metadata"]["ownerReferences"], json!([{
                "apiVersion": "control.suntitle.io/v1alpha1",
                "kind": "SuntitleHost",
                "name": host_name,
                "uid": host_uid,
                "controller": true,
            }]));
            assert!(node["metadata"]["finalizers"].is_null());
            assert_eq!(node["spec"]["hostRef"]["name"], host_name);
            assert!(node.get("status").is_none_or(Value::is_null));
        }
        assert!(hosts.get(&host_name).await.unwrap().finalizers().is_empty());
        let name = &created[0];
        let uri = format!("/api/v1/nodes/{name}");
        nodes.patch_status(name, &PatchParams::default(), &Patch::Merge(&json!({"status":{"phase":"Running","observedGeneration":1}}))).await.unwrap();
        let initial = expect(&store, Method::GET, &uri, None, 200).await;
        let mut spec = initial["spec"].clone(); spec["name"] = json!("Edited");
        let updated = expect(&store, Method::PUT, &uri, Some(json!({"spec":spec})), 200).await;
        assert_eq!(updated["status"], initial["status"]);
        assert_eq!(updated["metadata"]["ownerReferences"], initial["metadata"]["ownerReferences"]);
        // Kubernetes/CEL MUST reject invalid specs and immutable field changes.
        for changes in [json!({"hostRef":{"name":"other"}}), json!({"settings":{"display":variants["display"]}}), json!({"name":""}), json!({"settings":{"audio_source":{"sampleRateHz":12345,"channels":1,"encoding":"pcm_s16le"}}})] {
            let mut spec = updated["spec"].clone();
            for (k,v) in changes.as_object().unwrap() { spec[k] = v.clone(); }
            let error = expect(&store, Method::PUT, &uri, Some(json!({"spec":spec})), 422).await;
            assert_eq!(error["error"]["code"], "Invalid");
            assert!(error["error"]["details"]["causes"].is_array());
        }
        let restart_uri = format!("{uri}/restart");
        let first = expect(&store, Method::POST, &restart_uri, None, 202).await;
        let second = expect(&store, Method::POST, &restart_uri, None, 202).await;
        assert_ne!(first["spec"]["restartToken"], second["spec"]["restartToken"]);
        assert_eq!(second["status"], updated["status"]);
        assert_eq!(second["spec"]["name"], "Edited");
        // API updates MUST accept older desired data without a version precondition.
        let overwritten = expect(&store, Method::PUT, &uri, Some(json!({"spec":initial["spec"]})), 200).await;
        assert_eq!(overwritten["spec"], initial["spec"]);
        assert_eq!(overwritten["status"], updated["status"]);
        let duplicate = expect(&store, Method::POST, &format!("/api/v1/hosts/{host_uid}/nodes"), Some(json!({"name":name,"spec":initial["spec"]})), 409).await;
        assert_eq!(duplicate["error"]["code"], "AlreadyExists");
        expect(&store, Method::POST, "/api/v1/hosts/missing-host/nodes", Some(json!({"spec":initial["spec"]})), 404).await;
        expect(&store, Method::POST, "/api/v1/nodes/missing-node/restart", None, 404).await;
        nodes.patch(name, &PatchParams::default(), &Patch::Merge(&json!({"metadata":{"finalizers":[NODE_FINALIZER]}}))).await.unwrap();
        expect(&store, Method::DELETE, &uri, None, 202).await;
        expect(&store, Method::DELETE, &uri, None, 202).await;
        expect(&store, Method::DELETE, "/api/v1/nodes/missing-node", None, 404).await;
        let terminating = expect(&store, Method::POST, &restart_uri, None, 202).await;
        assert!(terminating["metadata"]["deletionTimestamp"].is_string());
        assert_eq!(terminating["metadata"]["finalizers"], json!([NODE_FINALIZER]));
        expect(&store, Method::PUT, &uri, Some(json!({"spec":initial["spec"]})), 200).await;
        hosts.patch(&host_name, &PatchParams::default(), &Patch::Merge(&json!({"metadata":{"finalizers":[HOST_FINALIZER]}}))).await.unwrap();
        hosts.delete(&host_name, &DeleteParams::default()).await.unwrap();
        let allocated = expect(&store, Method::POST, &format!("/api/v1/hosts/{host_uid}/nodes"), Some(json!({"spec":initial["spec"]})), 201).await;
        created.push(allocated["metadata"]["name"].as_str().unwrap().to_owned());
        assert!(allocated["metadata"]["finalizers"].is_null());
    }).catch_unwind().await;
    for name in &created {
        nodes
            .patch(
                name,
                &PatchParams::default(),
                &Patch::Merge(&json!({"metadata":{"finalizers":null}})),
            )
            .await
            .unwrap();
        if nodes.get_opt(name).await.unwrap().is_some() {
            nodes.delete(name, &DeleteParams::default()).await.unwrap();
        }
    }
    hosts
        .patch(
            &host_name,
            &PatchParams::default(),
            &Patch::Merge(&json!({"metadata":{"finalizers":null}})),
        )
        .await
        .unwrap();
    if hosts.get_opt(&host_name).await.unwrap().is_some() {
        hosts
            .delete(&host_name, &DeleteParams::default())
            .await
            .unwrap();
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[actix_web::test]
async fn api_server_errors_are_propagated_without_retry() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use warp::Filter;
    for code in [400, 403, 404, 409, 422, 429, 500, 503] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let upstream = json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"ServerReason","message":"Server validation context","details":{"name":"node"},"code":code});
        let reply = upstream.clone();
        let route =
            warp::method()
                .and(warp::body::bytes().map(|body: actix_web::web::Bytes| {
                    serde_json::from_slice::<Value>(&body).unwrap()
                }))
                .map(move |method: warp::http::Method, body: Value| {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(method, warp::http::Method::PATCH);
                    assert!(body["spec"]["restartToken"].is_string());
                    assert!(body.get("metadata").is_none());
                    warp::reply::with_status(
                        warp::reply::json(&reply),
                        warp::http::StatusCode::from_u16(code).unwrap(),
                    )
                });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let mut config = kube::Config::new(format!("http://{address}").parse().unwrap());
        config.default_retry = false;
        let client = Client::try_from(config).unwrap();
        let result = expect(
            &ResourceStore::new(client, "suntitle"),
            Method::POST,
            "/api/v1/nodes/node/restart",
            None,
            code,
        )
        .await;
        assert_eq!(result["error"]["code"], upstream["reason"]);
        assert_eq!(result["error"]["message"], upstream["message"]);
        assert_eq!(result["error"]["details"], upstream["details"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[actix_web::test]
async fn readiness_rejects_unestablished_crds() {
    use warp::Filter;
    let route = warp::any().map(|| warp::reply::json(&json!({
        "apiVersion":"apiextensions.k8s.io/v1","kind":"CustomResourceDefinition",
        "metadata":{"name":"suntitlehosts.control.suntitle.io"},
        "spec":{"group":"control.suntitle.io","names":{"kind":"SuntitleHost","plural":"suntitlehosts"},"scope":"Namespaced","versions":[]},
        "status":{"conditions":[{"type":"Established","status":"False","reason":"Installing","message":"Pending"}]}
    })));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(warp::serve(route).incoming(listener).run());
    let client = Client::try_from(kube::Config::new(
        format!("http://{address}").parse().unwrap(),
    ))
    .unwrap();
    expect(
        &ResourceStore::new(client, "suntitle"),
        Method::GET,
        "/readyz",
        None,
        503,
    )
    .await;
    server.abort();
}

#[actix_web::test]
async fn installation_resolves_host_uid_from_each_list_request() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use warp::Filter;

    let lists = Arc::new(AtomicUsize::new(0));
    let creates = Arc::new(AtomicUsize::new(0));
    let list_count = lists.clone();
    let create_count = creates.clone();
    let route = warp::method()
        .and(warp::path::full())
        .and(warp::body::bytes())
        .map(move |method: warp::http::Method, path: warp::path::FullPath, body: actix_web::web::Bytes| {
            let value = if method == warp::http::Method::GET {
                assert_eq!(path.as_str(), "/apis/control.suntitle.io/v1alpha1/namespaces/suntitle/suntitlehosts");
                list_count.fetch_add(1, Ordering::SeqCst);
                json!({"apiVersion":"control.suntitle.io/v1alpha1","kind":"SuntitleHostList","metadata":{"resourceVersion":"1"},"items":[
                    {"metadata":{"name":"other-host","uid":"other-uid"},"spec":{"name":"Other"}},
                    {"metadata":{"name":"target-host","uid":"target-uid"},"spec":{"name":"Target"}}
                ]})
            } else {
                assert_eq!(method, warp::http::Method::POST);
                assert_eq!(path.as_str(), "/apis/control.suntitle.io/v1alpha1/namespaces/suntitle/suntitlenodes");
                create_count.fetch_add(1, Ordering::SeqCst);
                let node: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(node["spec"]["hostRef"]["name"], "target-host");
                node
            };
            warp::reply::json(&value)
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(warp::serve(route).incoming(listener).run());
    let client = Client::try_from(kube::Config::new(
        format!("http://{address}").parse().unwrap(),
    ))
    .unwrap();
    let store = ResourceStore::new(client, "suntitle");
    let body = json!({"spec":{"name":"Screen","hostRef":{"name":"ignored"},"settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}}});
    for uid in ["target-uid", "target-uid", "target-host", "missing-uid"] {
        let status = if uid == "target-uid" { 201 } else { 404 };
        expect(
            &store,
            Method::POST,
            &format!("/api/v1/hosts/{uid}/nodes"),
            Some(body.clone()),
            status,
        )
        .await;
    }
    assert_eq!(lists.load(Ordering::SeqCst), 4);
    assert_eq!(creates.load(Ordering::SeqCst), 2);
    server.abort();
}

#[actix_web::test]
async fn extension_requests_persist_resources_and_refresh_only_the_request_token() {
    use std::sync::{Arc, Mutex};
    use warp::Filter;
    let stored = Arc::new(Mutex::new(None::<Value>));
    let tokens = Arc::new(Mutex::new(Vec::<String>::new()));
    let state = stored.clone();
    let refresh_tokens = tokens.clone();
    let route = warp::method().and(warp::path::full()).and(warp::body::bytes())
        .map(move |method: warp::http::Method, path: warp::path::FullPath, body: actix_web::web::Bytes| {
            let base = "/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitleextensions";
            let mut state = state.lock().unwrap();
            let (status, value) = match method {
                warp::http::Method::POST => {
                    assert_eq!(path.as_str(), base);
                    let mut value: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(value["kind"], "SuntitleExtension");
                    assert!(value.get("status").is_none() || value["status"].is_null());
                    if let Some(previous) = state.as_ref() {
                        assert_ne!(value["metadata"]["name"], previous["metadata"]["name"]);
                        let name = value["metadata"]["name"].as_str().unwrap();
                        assert!(name.starts_with("extension-"));
                        Uuid::parse_str(name.strip_prefix("extension-").unwrap()).unwrap();
                    }
                    value["metadata"]["uid"] = json!("extension-uid");
                    value["metadata"]["resourceVersion"] = json!("1");
                    value["metadata"]["generation"] = json!(1);
                    *state = Some(value.clone());
                    (201, value)
                },
                warp::http::Method::GET if path.as_str() == base => (200,
                    json!({"apiVersion":"control.suntitle.io/v1alpha1", "kind":"SuntitleExtensionList",
                        "metadata":{"resourceVersion":"12"}, "items":state.iter().cloned().collect::<Vec<_>>()})),
                warp::http::Method::GET => {
                    let value = state.as_ref().unwrap();
                    assert_eq!(path.as_str(), format!("{base}/{}", value["metadata"]["name"].as_str().unwrap()));
                    (200, value.clone())
                },
                warp::http::Method::PATCH => {
                    let value = state.as_mut().unwrap();
                    assert_eq!(path.as_str(), format!("{base}/{}", value["metadata"]["name"].as_str().unwrap()));
                    let patch: Value = serde_json::from_slice(&body).unwrap();
                    let token = patch["spec"]["refreshToken"].as_str().unwrap();
                    Uuid::parse_str(token).unwrap();
                    assert_eq!(patch, json!({"spec":{"refreshToken":token}}));
                    refresh_tokens.lock().unwrap().push(token.into());
                    value["spec"]["refreshToken"] = json!(token);
                    value["metadata"]["generation"] = json!(value["metadata"]["generation"].as_i64().unwrap() + 1);
                    (200, value.clone())
                },
                other => panic!("unexpected method: {other}"),
            };
            warp::reply::with_status(warp::reply::json(&value), warp::http::StatusCode::from_u16(status).unwrap())
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(warp::serve(route).incoming(listener).run());
    let client = Client::try_from(kube::Config::new(
        format!("http://{address}").parse().unwrap(),
    ))
    .unwrap();
    let store = ResourceStore::new(client, "test");
    let empty = expect(&store, Method::GET, "/api/v1/extensions", None, 200).await;
    assert_eq!(empty["items"], json!([]));
    assert_eq!(empty["resourceVersion"], "12");
    let body = serde_json::to_value(
        serde_json::from_str::<ws_proxy::kubernetes::store::ExtensionCreate>(include_str!(
            "../../mockserver/fixtures/kubernetes/extension-create.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let (status, created, location) = request(
        &store,
        Method::POST,
        "/api/v1/extensions",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        location.as_deref(),
        Some("/api/v1/extensions/custom-transcriber")
    );
    assert_eq!(created["spec"], body["spec"]);
    let publication = json!({"observedGeneration":1,"taskId":"old-task","artifactReference":"registry/example@sha256:old"});
    stored.lock().unwrap().as_mut().unwrap()["status"] = publication.clone();
    let fetched = expect(&store, Method::GET, location.as_deref().unwrap(), None, 200).await;
    assert_eq!(fetched["status"], publication);
    let list = expect(&store, Method::GET, "/api/v1/extensions", None, 200).await;
    assert_eq!(list["items"], json!([fetched]));
    for generation in [2, 3] {
        let refreshed = expect(
            &store,
            Method::POST,
            "/api/v1/extensions/custom-transcriber/refresh",
            None,
            202,
        )
        .await;
        assert_eq!(refreshed["metadata"]["generation"], generation);
        assert_eq!(refreshed["metadata"]["uid"], created["metadata"]["uid"]);
        assert_eq!(refreshed["status"], publication);
        let mut spec = refreshed["spec"].clone();
        spec.as_object_mut().unwrap().remove("refreshToken");
        assert_eq!(spec, body["spec"]);
    }
    {
        let tokens = tokens.lock().unwrap();
        assert_ne!(tokens[0], tokens[1]);
    }
    let mut generated = body;
    generated.as_object_mut().unwrap().remove("name");
    let (status, resource, location) =
        request(&store, Method::POST, "/api/v1/extensions", Some(generated)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        location.unwrap(),
        format!(
            "/api/v1/extensions/{}",
            resource["metadata"]["name"].as_str().unwrap()
        )
    );
    server.abort();
}

#[actix_web::test]
async fn extension_api_preserves_kubernetes_errors_and_rejects_malformed_requests() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use warp::Filter;
    let body = serde_json::to_value(
        serde_json::from_str::<ws_proxy::kubernetes::store::ExtensionCreate>(include_str!(
            "../../mockserver/fixtures/kubernetes/extension-create.json"
        ))
        .unwrap(),
    )
    .unwrap();
    for code in [403, 404, 409, 422, 503] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let upstream = json!({"apiVersion":"v1", "kind":"Status", "status":"Failure",
            "reason":"ServerReason", "message":"Extension storage error", "details":{"name":"plugin"}, "code":code});
        let reply = upstream.clone();
        let route = warp::any().map(move || {
            count.fetch_add(1, Ordering::SeqCst);
            warp::reply::with_status(
                warp::reply::json(&reply),
                warp::http::StatusCode::from_u16(code).unwrap(),
            )
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let mut config = kube::Config::new(format!("http://{address}").parse().unwrap());
        config.default_retry = false;
        let store = ResourceStore::new(Client::try_from(config).unwrap(), "test");
        for (index, (method, uri, body)) in [
            (Method::GET, "/api/v1/extensions", None),
            (Method::GET, "/api/v1/extensions/plugin", None),
            (Method::POST, "/api/v1/extensions", Some(body.clone())),
            (Method::POST, "/api/v1/extensions/plugin/refresh", None),
        ]
        .into_iter()
        .enumerate()
        {
            let result = expect(&store, method, uri, body, code).await;
            assert_eq!(result["error"]["code"], upstream["reason"]);
            assert_eq!(result["error"]["message"], upstream["message"]);
            assert_eq!(result["error"]["details"], upstream["details"]);
            assert_eq!(calls.load(Ordering::SeqCst), index + 1);
        }
        for malformed in [
            json!({}),
            json!({"source":"legacy","sourceType":"git"}),
            json!({"spec":{"name":"Missing adapter","sourceType":"git","source":"example"}}),
        ] {
            expect(
                &store,
                Method::POST,
                "/api/v1/extensions",
                Some(malformed),
                400,
            )
            .await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        server.abort();
    }
}

#[actix_web::test]
async fn readiness_requires_extension_crd_and_readable_collection() {
    use std::sync::{Arc, Mutex};
    use warp::Filter;
    for failure in ["crd", "collection", "none"] {
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = paths.clone();
        let route = warp::path::full().map(move |path: warp::path::FullPath| {
            let path = path.as_str();
            requests.lock().unwrap().push(path.into());
            let (code, value) = if path.contains("/customresourcedefinitions/") {
                let name = path.rsplit('/').next().unwrap();
                (200, json!({"apiVersion":"apiextensions.k8s.io/v1", "kind":"CustomResourceDefinition",
                    "metadata":{"name":name}, "spec":{"group":"control.suntitle.io", "names":{"kind":"SuntitleExtension","plural":"suntitleextensions"},"scope":"Namespaced","versions":[]},
                    "status":{"conditions":[{"type":"Established","status":if failure == "crd" && name.starts_with("suntitleextensions.") {"False"} else {"True"}}]}}))
            } else if failure == "collection" && path.ends_with("/suntitleextensions") {
                (403, json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","message":"Cannot list extensions","code":403}))
            } else {
                let kind = if path.ends_with("suntitlehosts") {"SuntitleHostList"}
                    else if path.ends_with("suntitlenodes") {"SuntitleNodeList"} else {"SuntitleExtensionList"};
                (200, json!({"apiVersion":"control.suntitle.io/v1alpha1","kind":kind,"metadata":{"resourceVersion":"1"},"items":[]}))
            };
            warp::reply::with_status(warp::reply::json(&value), warp::http::StatusCode::from_u16(code).unwrap())
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(route).incoming(listener).run());
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let store = ResourceStore::new(client, "test");
        let (status, _, _) = request(&store, Method::GET, "/readyz", None).await;
        assert_eq!(
            status.as_u16(),
            match failure {
                "crd" => 503,
                "collection" => 403,
                _ => 200,
            }
        );
        let paths = paths.lock().unwrap();
        assert!(paths.iter().any(|p| {
            p.ends_with("/customresourcedefinitions/suntitleextensions.control.suntitle.io")
        }));
        if failure != "crd" {
            assert!(
                paths
                    .iter()
                    .any(|p| p.ends_with("/namespaces/test/suntitleextensions"))
            );
        }
        server.abort();
    }
}
