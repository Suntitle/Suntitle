use super::{Context, ReconcileError, conditions, error_policy};
use crate::kubernetes::resources::*;
use futures_util::StreamExt;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{
    Api, ResourceExt,
    api::{Patch, PatchParams},
    runtime::{
        controller::{Action, Controller},
        reflector::ObjectRef,
        watcher,
    },
};
use std::sync::Arc;

pub(super) async fn run(
    nodes: Api<SuntitleNode>,
    extensions: Api<SuntitleExtension>,
    context: Arc<Context>,
) {
    let controller = Controller::new(nodes.clone(), watcher::Config::default());
    let store = controller.store();
    controller.watches(extensions.clone(), watcher::Config::default(), move |extension| {
        store.state().iter().filter(|node| node.spec.extension_ref.as_ref().is_some_and(|reference| reference.name == extension.name_any()))
            .map(|node| ObjectRef::from_obj(node.as_ref())).collect::<Vec<_>>()
    }).run(move |node, _| {
        let nodes = nodes.clone();
        let extensions = extensions.clone();
        async move {
            if node.metadata.deletion_timestamp.is_some() || node.spec.extension_ref.is_none() {
                return Ok(Action::await_change());
            }
            let extension = extensions.get_opt(&node.spec.extension_ref.as_ref().unwrap().name).await.map_err(|e| ReconcileError::Failed(e.into()))?;
            let previous = node.status.clone();
            let mut desired = (*node).clone();
            resolve(&mut desired, extension.as_ref());
            let status = desired.status.as_mut().unwrap();
            conditions::merge(previous.as_ref().map(|s| s.conditions.as_slice()).unwrap_or_default(), &mut status.conditions);
            if serde_json::json!(previous.as_ref()) != serde_json::json!(Some(&*status)) {
                nodes.patch_status(&node.name_any(), &PatchParams::default(), &Patch::Merge(serde_json::json!({
                    "metadata":{"uid":node.uid(),"resourceVersion":node.resource_version()},
                    "status":{
                        "resolvedAdapter":status.resolved_adapter,
                        "artifactReference":status.artifact_reference,
                        "phase":status.phase,
                        "message":status.message,
                        "conditions":status.conditions
                    }
                }))).await.map_err(|e| ReconcileError::Failed(e.into()))?;
            }
            Ok(Action::await_change())
        }
    }, error_policy, context).for_each(|result| async move {
        if let Err(error) = result { eprintln!("extension controller: {error}"); }
    }).await;
}

fn resolve(node: &mut SuntitleNode, extension: Option<&SuntitleExtension>) {
    let status = node.status.get_or_insert_default();
    status.resolved_adapter = None;
    status.artifact_reference = None;
    let outcome = (|| {
        let extension = extension
            .filter(|e| e.metadata.deletion_timestamp.is_none())
            .ok_or((
                "ExtensionUnavailable",
                "Referenced extension is missing or being deleted.".to_string(),
            ))?;
        let resolved = node
            .spec
            .resolve_extension(&extension.spec)
            .map_err(|error| ("InvalidExtensionSettings", error.to_string()))?;
        let published = extension
            .status
            .as_ref()
            .filter(|s| {
                s.observed_generation == extension.metadata.generation
                    && s.conditions.iter().any(|c| {
                        c.type_ == "Ready"
                            && c.status == "True"
                            && c.observed_generation == extension.metadata.generation
                    })
            })
            .and_then(|s| s.artifact_reference.as_ref())
            .filter(|reference| !reference.is_empty())
            .ok_or((
                "ExtensionNotReady",
                "Waiting for the current extension artifact to be published.".to_string(),
            ))?;
        Ok((resolved, published.clone()))
    })();
    let (reason, message, phase) = match outcome {
        Ok((resolved, reference)) => {
            status.resolved_adapter = resolved;
            status.artifact_reference = Some(reference);
            (
                "Preparing",
                "Extension artifact resolved; waiting for host installation.".to_string(),
                NodePhase::Pending,
            )
        }
        Err((reason, message)) => {
            let phase = if reason == "InvalidExtensionSettings" {
                NodePhase::Failed
            } else {
                NodePhase::Pending
            };
            (reason, message, phase)
        }
    };
    status.phase = Some(phase);
    status.message = Some(message.clone());
    for kind in ["Installed", "Ready", "Updated"] {
        status.conditions.push(Condition {
            type_: kind.into(),
            status: "False".into(),
            reason: reason.into(),
            message: message.clone(),
            observed_generation: node.metadata.generation,
            last_transition_time: Time(k8s_openapi::jiff::Timestamp::now()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node() -> SuntitleNode {
        serde_json::from_value(json!({"metadata":{"name":"node","generation":2},"spec":{
            "name":"Node","hostRef":{"name":"host"},"extensionRef":{"name":"extension"},
            "settings":{"custom":{"language":"cs"}}
        }}))
        .unwrap()
    }

    fn extension() -> SuntitleExtension {
        serde_json::from_value(json!({"metadata":{"name":"extension","generation":3},"spec":{
            "name":"Extension","sourceType":"local","source":"/extension","adapter":{
                "type":"transcriber","execution":{"runtime":"executable"},
                "settings":{"language":{"required":true},"threads":{"default":4}}
            }
        },"status":{"observedGeneration":3,"artifactReference":"registry/extensions@sha256:abc",
            "conditions":[{"type":"Ready","status":"True","observedGeneration":3,
                "reason":"ArtifactPublished","message":"Published","lastTransitionTime":"2026-11-01T00:00:00Z"}]
        }})).unwrap()
    }

    #[test]
    fn resolves_current_publication_without_claiming_installation() {
        let mut node = node();
        let spec = json!(node.spec);
        resolve(&mut node, Some(&extension()));
        let status = node.status.unwrap();
        let adapter = status.resolved_adapter.unwrap();
        assert_eq!(adapter.settings["language"], "cs");
        assert_eq!(adapter.settings["threads"], 4);
        assert!(status.artifact_reference.is_some());
        assert!(
            status
                .conditions
                .iter()
                .all(|c| c.status == "False" && c.observed_generation == Some(2))
        );
        assert_eq!(json!(node.spec), spec);
    }

    #[test]
    fn refresh_and_deletion_clear_obsolete_resolution() {
        let mut node = node();
        let mut extension = extension();
        resolve(&mut node, Some(&extension));
        extension.metadata.generation = Some(4);
        resolve(&mut node, Some(&extension));
        let status = node.status.as_ref().unwrap();
        assert!(status.resolved_adapter.is_none());
        assert!(status.artifact_reference.is_none());
        assert_eq!(
            status.conditions.last().unwrap().reason,
            "ExtensionNotReady"
        );
        resolve(&mut node, None);
        assert_eq!(
            node.status.unwrap().conditions.last().unwrap().reason,
            "ExtensionUnavailable"
        );
    }

    #[test]
    fn missing_required_override_fails_and_recovers() {
        let mut node = node();
        node.spec.settings = NodeSettings::Custom(Default::default());
        resolve(&mut node, Some(&extension()));
        assert_eq!(
            node.status
                .as_ref()
                .unwrap()
                .conditions
                .last()
                .unwrap()
                .reason,
            "InvalidExtensionSettings"
        );
        assert!(node.status.as_ref().unwrap().artifact_reference.is_none());
        node.spec.settings = serde_json::from_value(json!({"custom":{"language":"cs"}})).unwrap();
        resolve(&mut node, Some(&extension()));
        assert!(node.status.unwrap().artifact_reference.is_some());
    }
}
