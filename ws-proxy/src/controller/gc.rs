//! Host and extension deletion garbage collection.
//! Parent deletion requests node removal, then waits for node finalizers to finish.
//! Offline hosts defer completion. Errors retain the parent finalizer for retry.
use super::{Context, ReconcileError, error_policy, finalizer_error, session::State};
use futures_util::StreamExt;
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, ListParams},
    runtime::{
        controller::{Action, Config, Controller as KubeController},
        finalizer::{self, Event},
        watcher,
    },
};
use std::sync::Arc;
use suntitle_core::kubernetes::{Error, resources::*};

/// Delete nodes referencing a terminating host.
pub(super) async fn clean_nodes(hosts: Api<SuntitleHost>, context: Arc<Context>) {
    KubeController::new(hosts, watcher::Config::default())
        .with_config(Config::default().concurrency(1))
        .run(reconcile, error_policy, context)
        .for_each(|result| async move {
            if let Err(error) = result {
                eprintln!("host GC controller: {error}");
            }
        })
        .await;
}

/// Hold the host finalizer until dependent nodes disappear.
async fn reconcile(host: Arc<SuntitleHost>, ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    let mut state = ctx.state.lock().await;
    let api = state.hosts_api.clone();
    finalizer::finalizer(&api, HOST_FINALIZER, host, |event| async {
        match event {
            Event::Apply(_) => Ok(Action::await_change()),
            Event::Cleanup(host) => {
                if state
                    .cleanup_host(&host)
                    .await
                    .map_err(ReconcileError::Failed)?
                {
                    Ok(Action::await_change())
                } else {
                    Err(ReconcileError::CleanupPending)
                }
            }
        }
    })
    .await
    .map_err(finalizer_error)
}

impl State {
    /// Delete dependent nodes and report whether cleanup finished.
    async fn cleanup_host(&mut self, host: &SuntitleHost) -> Result<bool, Error> {
        let nodes = self.nodes_api.list(&ListParams::default()).await?;
        let referencing: Vec<_> = nodes
            .iter()
            .filter(|node| node.spec.host_ref.name == host.name_any())
            .collect();
        if referencing.is_empty() {
            self.close(host.metadata.uid.as_deref().unwrap());
            return Ok(true);
        }
        for node in referencing {
            match self
                .nodes_api
                .delete(&node.name_any(), &DeleteParams::default())
                .await
            {
                Ok(_) => {}
                Err(kube::Error::Api(error)) if error.code == 404 => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }
}

/// Delete nodes referencing a terminating extension.
pub(super) async fn clean_extensions(extensions: Api<SuntitleExtension>, context: Arc<Context>) {
    let api = extensions.clone();
    KubeController::new(extensions, watcher::Config::default())
        .with_config(Config::default().concurrency(1))
        .run(
            move |extension, ctx| reconcile_extension(extension, api.clone(), ctx),
            error_policy,
            context,
        )
        .for_each(|result| async move {
            if let Err(error) = result {
                eprintln!("extension GC controller: {error}");
            }
        })
        .await;
}

/// Hold the extension finalizer until dependent nodes disappear.
async fn reconcile_extension(
    extension: Arc<SuntitleExtension>,
    api: Api<SuntitleExtension>,
    ctx: Arc<Context>,
) -> Result<Action, ReconcileError> {
    let mut state = ctx.state.lock().await;
    finalizer::finalizer(&api, EXTENSION_FINALIZER, extension, |event| async {
        match event {
            Event::Apply(_) => Ok(Action::await_change()),
            Event::Cleanup(extension) => {
                if state
                    .cleanup_extension(&extension)
                    .await
                    .map_err(ReconcileError::Failed)?
                {
                    Ok(Action::await_change())
                } else {
                    Err(ReconcileError::CleanupPending)
                }
            }
        }
    })
    .await
    .map_err(finalizer_error)
}

impl State {
    /// Delete extension dependents and report whether cleanup finished.
    async fn cleanup_extension(&mut self, extension: &SuntitleExtension) -> Result<bool, Error> {
        let nodes = self.nodes_api.list(&ListParams::default()).await?;
        let referencing: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.spec
                    .extension_ref
                    .as_ref()
                    .is_some_and(|reference| reference.name == extension.name_any())
            })
            .collect();
        for node in &referencing {
            match self
                .nodes_api
                .delete(&node.name_any(), &DeleteParams::default())
                .await
            {
                Ok(_) => {}
                Err(kube::Error::Api(error)) if error.code == 404 => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(referencing.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use warp::Filter;

    /// Verify host cleanup waits for all referencing nodes.
    #[tokio::test]
    async fn cleanup_selects_host_ref_and_waits_for_terminating_nodes() {
        let matching = super::super::tests::node();
        let mut terminating = matching.clone();
        terminating.metadata.name = Some("terminating".into());
        terminating.metadata.deletion_timestamp =
            Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            ));
        let mut unrelated = matching.clone();
        unrelated.metadata.name = Some("unrelated".into());
        unrelated.spec.host_ref.name = "another-host".into();
        let resources = Arc::new(std::sync::Mutex::new(vec![
            matching,
            terminating,
            unrelated,
        ]));
        let listed = resources.clone();
        let list = warp::get().map(move || {
            warp::reply::json(&json!({
                "apiVersion": "control.suntitle.io/v1alpha1",
                "kind": "SuntitleNodeList",
                "metadata": {"resourceVersion": "1"},
                "items": *listed.lock().unwrap()
            }))
        });
        let deleted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = deleted.clone();
        let delete =
            warp::delete()
                .and(warp::path::full())
                .map(move |path: warp::path::FullPath| {
                    recorded.lock().unwrap().push(path.as_str().to_owned());
                    warp::reply::with_status(
                        warp::reply::json(&json!({
                            "apiVersion": "v1", "kind": "Status", "status": "Failure",
                            "reason": "NotFound", "message": "already deleted", "code": 404
                        })),
                        warp::http::StatusCode::NOT_FOUND,
                    )
                });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(list.or(delete)).incoming(listener).run());
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let mut state = State {
            hosts_api: Api::namespaced(client.clone(), "test"),
            nodes_api: Api::namespaced(client, "test"),
            connections: Default::default(),
        };
        let mut host = SuntitleHost::new(
            "host",
            SuntitleHostSpec {
                display_name: "Host".into(),
            },
        );
        host.metadata.uid = Some("host-uid".into());
        assert!(!state.cleanup_host(&host).await.unwrap());
        assert_eq!(
            *deleted.lock().unwrap(),
            vec![
                "/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/screen",
                "/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/terminating",
            ]
        );
        resources
            .lock()
            .unwrap()
            .retain(|node| node.spec.host_ref.name != "host");
        assert!(state.cleanup_host(&host).await.unwrap());
        server.abort();
    }

    /// Extension cleanup follows spec references across hosts and retries failed deletions.
    #[tokio::test]
    async fn extension_cleanup_waits_for_referenced_nodes_and_preserves_unrelated_nodes() {
        use std::sync::atomic::{AtomicU16, Ordering};
        let mut matching = super::super::tests::node();
        matching.spec.extension_ref = Some(ResourceRef {
            name: "plugin".into(),
        });
        matching.spec.settings = NodeSettings::Custom(Default::default());
        let mut other_host = matching.clone();
        other_host.metadata.name = Some("other-host-node".into());
        other_host.spec.host_ref.name = "another-host".into();
        let mut terminating = matching.clone();
        terminating.metadata.name = Some("terminating".into());
        terminating.metadata.deletion_timestamp =
            Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                k8s_openapi::jiff::Timestamp::now(),
            ));
        let mut unrelated = matching.clone();
        unrelated.metadata.name = Some("unrelated".into());
        unrelated.spec.extension_ref.as_mut().unwrap().name = "other-plugin".into();
        let mut builtin = super::super::tests::node();
        builtin.metadata.name = Some("builtin".into());
        let resources = Arc::new(std::sync::Mutex::new(vec![
            matching,
            other_host,
            terminating,
            unrelated,
            builtin,
        ]));
        let listed = resources.clone();
        let list_status = Arc::new(AtomicU16::new(200));
        let listed_status = list_status.clone();
        let list = warp::get().map(move || {
            let code = listed_status.load(Ordering::SeqCst);
            let value = if code == 200 {
                json!({"apiVersion":"control.suntitle.io/v1alpha1", "kind":"SuntitleNodeList",
                    "metadata":{"resourceVersion":"1"}, "items":*listed.lock().unwrap()})
            } else {
                json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","message":"Cannot list nodes","code":code})
            };
            warp::reply::with_status(warp::reply::json(&value), warp::http::StatusCode::from_u16(code).unwrap())
        });
        let deleted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = deleted.clone();
        let delete_status = Arc::new(AtomicU16::new(403));
        let recorded_status = delete_status.clone();
        let delete = warp::delete().and(warp::path::full()).map(move |path: warp::path::FullPath| {
            recorded.lock().unwrap().push(path.as_str().to_owned());
            let code = recorded_status.load(Ordering::SeqCst);
            warp::reply::with_status(warp::reply::json(&json!({
                "apiVersion":"v1","kind":"Status","status":if code == 200 {"Success"} else {"Failure"},
                "reason":"TestResponse","message":"Delete node","code":code
            })), warp::http::StatusCode::from_u16(code).unwrap())
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(warp::serve(list.or(delete)).incoming(listener).run());
        let mut config = kube::Config::new(format!("http://{address}").parse().unwrap());
        config.default_retry = false;
        let client = kube::Client::try_from(config).unwrap();
        let mut state = State {
            hosts_api: Api::namespaced(client.clone(), "test"),
            nodes_api: Api::namespaced(client, "test"),
            connections: Default::default(),
        };
        let extension = SuntitleExtension::new(
            "plugin",
            serde_json::from_value(json!({
                "displayName":"Plugin", "sourceType":"local", "source":"/extension",
                "adapter":{"type":"display","execution":{"runtime":"executable","command":"./run"}}
            }))
            .unwrap(),
        );
        assert!(state.cleanup_extension(&extension).await.is_err());
        deleted.lock().unwrap().clear();
        for code in [404, 200] {
            delete_status.store(code, Ordering::SeqCst);
            assert!(!state.cleanup_extension(&extension).await.unwrap());
            assert_eq!(
                *deleted.lock().unwrap(),
                vec![
                    "/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/screen",
                    "/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/other-host-node",
                    "/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/terminating",
                ]
            );
            deleted.lock().unwrap().clear();
        }
        resources.lock().unwrap().retain(|node| {
            node.spec
                .extension_ref
                .as_ref()
                .is_none_or(|r| r.name != "plugin")
                || node.metadata.deletion_timestamp.is_some()
        });
        assert!(!state.cleanup_extension(&extension).await.unwrap());
        assert_eq!(
            *deleted.lock().unwrap(),
            vec!["/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/terminating",]
        );
        resources
            .lock()
            .unwrap()
            .retain(|node| node.metadata.deletion_timestamp.is_none());
        list_status.store(403, Ordering::SeqCst);
        assert!(state.cleanup_extension(&extension).await.is_err());
        list_status.store(200, Ordering::SeqCst);
        assert!(state.cleanup_extension(&extension).await.unwrap());
        server.abort();
    }
}
