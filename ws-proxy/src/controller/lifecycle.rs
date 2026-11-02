//! Project validated host acknowledgements directly into Node status.
//!
//! Successful preparation writes Installed=True and Pending/AwaitingLaunch together.
//! It MUST NOT report running. Reconciliation then selects custom apply; only actual
//! spawn success reports Running. Disabled apply reports Disabled. Preparation and
//! apply failures report Failed. Removal stays Terminating until cleanup releases
//! the finalizer. Only successful acknowledgements observe the desired revision.
use crate::{
    kubernetes::resources::{NodePhase, SuntitleNode},
    registration::Operation,
};

/// Write owned conditions once while preserving unrelated conditions and diagnostics.
/// Installation is Some(true) for prepared files, Some(false) for preparation failure,
/// and None when the operation MUST retain the existing installation observation.
fn report(node: &mut SuntitleNode, phase: NodePhase, installation: Option<bool>) {
    let generation = node.metadata.generation.unwrap();
    let status = node.status.as_mut().unwrap();
    let mut conditions = vec![
        phase.ready_condition(generation, status.message.as_deref()),
        phase.updated_condition(generation, status.message.as_deref()),
    ];
    if let Some(installed) = installation {
        let reason = if installed {
            "AwaitingLaunch"
        } else {
            "ExtensionPreparationFailed"
        };
        for condition in &mut conditions {
            condition.reason = reason.into();
            if status.message.is_none() {
                condition.message = "Argument construction and process launch are pending.".into();
            }
        }
        let mut condition = conditions[0].clone();
        condition.type_ = "Installed".into();
        condition.status = if installed { "True" } else { "False" }.into();
        if installed {
            condition.reason = "ExtensionInstalled".into();
        }
        conditions.push(condition);
    }
    for condition in conditions {
        match status
            .conditions
            .iter_mut()
            .find(|previous| previous.type_ == condition.type_)
        {
            Some(previous) => *previous = condition,
            None => status.conditions.push(condition),
        }
    }
    status.phase = Some(phase);
}

/// Project a dispatch or connection phase without changing installation status.
/// Node status and generation MUST be present.
pub(super) fn set_phase(node: &mut SuntitleNode, phase: NodePhase) {
    report(node, phase, None);
}

/// Apply a validated operation result. Resource, generation, operation, artifact,
/// and session MUST be checked by the caller. Node status MUST contain its dispatch.
pub(super) fn acknowledge(node: &mut SuntitleNode, operation: Operation, success: bool) {
    if success {
        let status = node.status.as_mut().unwrap();
        let dispatch = status.dispatch.as_ref().unwrap();
        status.observed_generation = Some(dispatch.generation);
        status.reporting_session = dispatch.session_id.clone();
        status.acknowledged_restart_token = node.spec.restart_token.clone();
    }
    let phase = match (operation, success, node.spec.enabled) {
        (Operation::Remove, _, _) => NodePhase::Terminating,
        (_, false, _) => NodePhase::Failed,
        (Operation::Download, true, _) => NodePhase::Pending,
        (Operation::Apply, true, true) => NodePhase::Running,
        (Operation::Apply, true, false) => NodePhase::Disabled,
    };
    report(
        node,
        phase,
        (operation == Operation::Download).then_some(success),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Acknowledgements preserve operation semantics and replace owned conditions.
    #[test]
    fn repeated_reports_keep_conditions_unique_and_only_success_observes_a_revision() {
        for operation in [Operation::Download, Operation::Apply, Operation::Remove] {
            for success in [false, true] {
                for enabled in [false, true] {
                    let mut node = super::super::tests::node();
                    node.spec.enabled = enabled;
                    node.status = Some(serde_json::from_value(json!({
                        "dispatch": {"generation": 1, "operation": operation, "operationId": "operation", "sessionId": "session"},
                        "conditions": [{
                            "type":"External", "status":"True", "reason":"Retained",
                            "message":"Owned by another observer", "observedGeneration":1,
                            "lastTransitionTime":"2026-11-01T10:00:00Z"
                        }]
                    })).unwrap());
                    acknowledge(&mut node, operation, success);
                    acknowledge(&mut node, operation, success);
                    let status = node.status.unwrap();
                    let expected = match (operation, success, enabled) {
                        (Operation::Remove, _, _) => NodePhase::Terminating,
                        (_, false, _) => NodePhase::Failed,
                        (Operation::Download, true, _) => NodePhase::Pending,
                        (Operation::Apply, true, true) => NodePhase::Running,
                        (Operation::Apply, true, false) => NodePhase::Disabled,
                    };
                    assert_eq!(serde_json::json!(status.phase), serde_json::json!(expected));
                    assert_eq!(status.observed_generation, success.then_some(1));
                    assert_eq!(
                        status.conditions.len(),
                        if operation == Operation::Download {
                            4
                        } else {
                            3
                        }
                    );
                    if operation == Operation::Download {
                        assert!(
                            status
                                .conditions
                                .iter()
                                .filter(|condition| matches!(
                                    condition.type_.as_str(),
                                    "Ready" | "Updated"
                                ))
                                .all(|condition| condition.status == "False")
                        );
                        assert_eq!(
                            status
                                .conditions
                                .iter()
                                .find(|condition| condition.type_ == "Installed")
                                .unwrap()
                                .status,
                            if success { "True" } else { "False" }
                        );
                    }
                    let external = status
                        .conditions
                        .iter()
                        .find(|condition| condition.type_ == "External")
                        .unwrap();
                    assert_eq!(external.reason, "Retained");
                    assert_eq!(external.message, "Owned by another observer");
                }
            }
        }
    }
}
