//! Host deletion garbage collection.
use super::{Context, ReconcileError, error_policy, finalizer_error, session::State};
use crate::kubernetes::{Error, resources::*};
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

/// Delete nodes referencing a terminating host.
pub(super) async fn run(hosts: Api<SuntitleHost>, context: Arc<Context>) {
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
            if node.metadata.deletion_timestamp.is_none() {
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
        }
        Ok(false)
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
                name: "Host".into(),
            },
        );
        host.metadata.uid = Some("host-uid".into());
        assert!(!state.cleanup_host(&host).await.unwrap());
        assert_eq!(
            *deleted.lock().unwrap(),
            vec!["/apis/control.suntitle.io/v1alpha1/namespaces/test/suntitlenodes/screen"]
        );
        resources
            .lock()
            .unwrap()
            .retain(|node| node.spec.host_ref.name != "host");
        assert!(state.cleanup_host(&host).await.unwrap());
        server.abort();
    }
}
