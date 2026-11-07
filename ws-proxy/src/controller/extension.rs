//! Resolve publication and command configuration independently.
//! A new artifact MUST invalidate installation before download and apply.
//! Configuration changes MUST retain installation and invalidate only apply.
use super::{Context, ReconcileError, conditions, error_policy};
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
use suntitle_core::kubernetes::resources::*;

/// Watch nodes and referenced extension publications.
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
                        "dispatch":status.dispatch,
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

/// Resolve current configuration while preserving installation for unchanged payloads.
fn resolve(node: &mut SuntitleNode, extension: Option<&SuntitleExtension>) {
    let status = node.status.get_or_insert_default();
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
            let unchanged = status.artifact_reference.as_ref() == Some(&reference)
                && serde_json::json!(status.resolved_adapter) == serde_json::json!(resolved)
                && status.conditions.iter().any(|c| {
                    c.type_ == "Installed" && c.observed_generation == node.metadata.generation
                });
            if unchanged {
                return;
            }
            let installed = status.artifact_reference.as_ref() == Some(&reference)
                && status
                    .conditions
                    .iter()
                    .any(|c| c.type_ == "Installed" && c.status == "True");
            status.dispatch = None;
            status.resolved_adapter = resolved;
            status.artifact_reference = Some(reference);
            if installed {
                for condition in &mut status.conditions {
                    if condition.type_ == "Installed" {
                        condition.observed_generation = node.metadata.generation;
                    }
                }
                status.phase = Some(NodePhase::Pending);
                status.message =
                    Some("Command configuration resolved; waiting for runtime apply.".into());
                return;
            }
            (
                "Preparing",
                "Extension artifact resolved; waiting for host installation.".to_string(),
                NodePhase::Pending,
            )
        }
        Err((reason, message)) => {
            status.resolved_adapter = None;
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
        if kind == "Installed" && status.resolved_adapter.is_none() {
            continue;
        }
        status
            .conditions
            .retain(|condition| condition.type_ != kind);
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
            "displayName":"Node","hostRef":{"name":"host"},"extensionRef":{"name":"extension"},
            "settings":{"custom":{"language":"cs"}}
        }}))
        .unwrap()
    }

    fn extension() -> SuntitleExtension {
        serde_json::from_value(json!({"metadata":{"name":"extension","generation":3},"spec":{
            "displayName":"Extension","sourceType":"local","source":"/extension","adapter":{
                "type":"transcriber","execution":{"runtime":"executable","command":"./run"},
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
    fn refresh_and_deletion_block_dispatch_while_retaining_payload_identity() {
        let mut node = node();
        let mut extension = extension();
        resolve(&mut node, Some(&extension));
        extension.metadata.generation = Some(4);
        resolve(&mut node, Some(&extension));
        let status = node.status.as_ref().unwrap();
        assert!(status.resolved_adapter.is_none());
        assert!(status.artifact_reference.is_some());
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
    /// Settings and command revisions preserve installation; only changed content invalidates it.
    #[test]
    fn configuration_changes_reuse_installation_and_content_changes_require_bootstrap() {
        let mut node = node();
        let mut extension = extension();
        resolve(&mut node, Some(&extension));
        let installed = node
            .status
            .as_mut()
            .unwrap()
            .conditions
            .iter_mut()
            .find(|c| c.type_ == "Installed")
            .unwrap();
        installed.status = "True".into();
        node.metadata.generation = Some(3);
        resolve(&mut node, Some(&extension));
        assert!(matches!(
            super::super::plan::Plan::for_node(&node),
            Some(super::super::plan::Plan::CustomApply { .. })
        ));
        extension.spec.adapter.execution =
            serde_json::from_value(json!({"command":"./other"})).unwrap();
        resolve(&mut node, Some(&extension));
        assert!(matches!(
            super::super::plan::Plan::for_node(&node),
            Some(super::super::plan::Plan::CustomApply { .. })
        ));
        extension.metadata.generation = Some(4);
        resolve(&mut node, Some(&extension));
        assert!(super::super::plan::Plan::for_node(&node).is_none());
        extension.status.as_mut().unwrap().observed_generation = Some(4);
        extension.status.as_mut().unwrap().conditions[0].observed_generation = Some(4);
        resolve(&mut node, Some(&extension));
        assert!(matches!(
            super::super::plan::Plan::for_node(&node),
            Some(super::super::plan::Plan::CustomApply { .. })
        ));
        extension.status.as_mut().unwrap().artifact_reference =
            Some("registry/extensions@sha256:new".into());
        resolve(&mut node, Some(&extension));
        assert!(matches!(
            super::super::plan::Plan::for_node(&node),
            Some(super::super::plan::Plan::Download { .. })
        ));
    }

    #[test]
    fn unchanged_resolution_preserves_download_report_and_new_artifacts_reset_it() {
        let mut node = node();
        let mut extension = extension();
        resolve(&mut node, Some(&extension));
        node.status.as_mut().unwrap().message = Some("download completed".into());
        resolve(&mut node, Some(&extension));
        assert_eq!(
            node.status.as_ref().unwrap().message.as_deref(),
            Some("download completed")
        );
        extension.status.as_mut().unwrap().artifact_reference =
            Some("registry/extensions@sha256:def".into());
        resolve(&mut node, Some(&extension));
        assert_ne!(
            node.status.as_ref().unwrap().message.as_deref(),
            Some("download completed")
        );
        assert!(node.status.as_ref().unwrap().dispatch.is_none());
    }
}
