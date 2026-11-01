//! Ordered transitions from validated host acknowledgements to CR observations.
//!
//! | Operation | Successful transition | Failed transition |
//! | --- | --- | --- |
//! | Download | `Downloading` → `Downloaded` → `AwaitingPrerequisites` → `Pending` | `Downloading` → `Failed` |
//! | Built-in apply | Acknowledged → `Running` or `Disabled` | Rejected → `Failed` |
//! | Remove | Acknowledged → `Terminating` | Rejected → `Terminating` |
//!
//! `Downloaded` MUST advance to `AwaitingPrerequisites` before reporting. Neither
//! state permits execution. Prerequisite preparation remains deferred. Built-in
//! apply bypasses extension preparation. Removal MUST retain its finalizer until
//! cleanup reconciliation verifies success. Only successful acknowledgements
//! record the observed revision. Every transition writes its final conditions once.
use crate::{
    kubernetes::resources::{NodePhase, SuntitleNode},
    registration::Operation,
};

/// Typed result decoded from the wire acknowledgement.
#[derive(Clone, Copy)]
enum Outcome {
    /// The host completed the dispatched operation.
    Success,
    /// The host could not complete the dispatched operation.
    Failure,
}

/// A dispatched artifact transfer awaiting its host result.
struct Downloading<'a> {
    /// Node with a validated current dispatch and host session.
    node: &'a mut SuntitleNode,
}

/// Verified artifact files. This state cannot report execution readiness.
struct Downloaded<'a> {
    /// Node whose artifact transfer completed successfully.
    node: &'a mut SuntitleNode,
}

/// Downloaded files awaiting the deferred prerequisite preparation stage.
struct AwaitingPrerequisites<'a> {
    /// Node whose files are available but whose installation is incomplete.
    node: &'a mut SuntitleNode,
}

/// Final status projection shared by all operation outcomes.
struct Observation<'a> {
    /// Resource whose existing diagnostic and generation are preserved.
    node: &'a mut SuntitleNode,
    /// Phase selected by the operation transition.
    phase: NodePhase,
    /// Download outcome overriding readiness reasons and adding Installed=False.
    download_reason: Option<&'static str>,
}

impl<'a> Downloading<'a> {
    /// Consume the host result. Only success produces the downloaded state.
    fn acknowledge(self, outcome: Outcome) -> Result<Downloaded<'a>, Observation<'a>> {
        match outcome {
            Outcome::Success => Ok(Downloaded { node: self.node }),
            Outcome::Failure => Err(Observation {
                node: self.node,
                phase: NodePhase::Failed,
                download_reason: Some("ArtifactDownloadFailed"),
            }),
        }
    }
}

impl<'a> Downloaded<'a> {
    /// Advance files to the prerequisite boundary without claiming installation.
    fn await_prerequisites(self) -> AwaitingPrerequisites<'a> {
        AwaitingPrerequisites { node: self.node }
    }
}

impl AwaitingPrerequisites<'_> {
    /// Report the completed download while keeping installation and readiness false.
    fn report(self) {
        Observation {
            node: self.node,
            phase: NodePhase::Pending,
            download_reason: Some("ArtifactDownloaded"),
        }
        .report();
    }
}

impl<'a> Observation<'a> {
    /// Project a phase using the standard readiness and update conditions.
    fn new(node: &'a mut SuntitleNode, phase: NodePhase) -> Self {
        Self {
            node,
            phase,
            download_reason: None,
        }
    }

    /// Replace each owned condition once with its final transition value.
    /// Unrelated conditions remain. Persistence preserves transition timestamps.
    fn report(self) {
        let generation = self.node.metadata.generation.unwrap();
        let status = self.node.status.as_mut().unwrap();
        let mut conditions = vec![
            self.phase
                .ready_condition(generation, status.message.as_deref()),
            self.phase
                .updated_condition(generation, status.message.as_deref()),
        ];
        if let Some(reason) = self.download_reason {
            for condition in &mut conditions {
                condition.reason = reason.into();
                if status.message.is_none() {
                    condition.message =
                        "Prerequisite preparation and execution are deferred.".into();
                }
            }
            let mut installed = conditions[0].clone();
            installed.type_ = "Installed".into();
            conditions.push(installed);
        }
        for condition in conditions {
            match status
                .conditions
                .iter_mut()
                .find(|c| c.type_ == condition.type_)
            {
                Some(previous) => *previous = condition,
                None => status.conditions.push(condition),
            }
        }
        status.phase = Some(self.phase);
    }
}

/// Project a dispatch or connection phase without rebuilding operation decisions.
/// Node status and generation MUST be present.
pub(super) fn set_phase(node: &mut SuntitleNode, phase: NodePhase) {
    Observation::new(node, phase).report();
}

/// Decode a validated acknowledgement and follow its operation's transition order.
/// Node status MUST contain the current dispatch. The caller MUST validate the
/// resource, generation, operation, artifact, and session before entering here.
pub(super) fn acknowledge(node: &mut SuntitleNode, operation: Operation, success: bool) {
    let outcome = match success {
        true => Outcome::Success,
        false => Outcome::Failure,
    };
    if let Outcome::Success = outcome {
        let status = node.status.as_mut().unwrap();
        let dispatch = status.dispatch.as_ref().unwrap();
        status.observed_generation = Some(dispatch.generation);
        status.reporting_session = dispatch.session_id.clone();
        status.acknowledged_restart_token = node.spec.restart_token.clone();
    }
    match operation {
        Operation::Download => match (Downloading { node }).acknowledge(outcome) {
            Ok(downloaded) => downloaded.await_prerequisites().report(),
            Err(failed) => failed.report(),
        },
        Operation::Apply => {
            let phase = match (outcome, node.spec.enabled) {
                (Outcome::Success, true) => NodePhase::Running,
                (Outcome::Success, false) => NodePhase::Disabled,
                (Outcome::Failure, _) => NodePhase::Failed,
            };
            Observation::new(node, phase).report();
        }
        Operation::Remove => Observation::new(node, NodePhase::Terminating).report(),
    }
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
                        "conditions": []
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
                            3
                        } else {
                            2
                        }
                    );
                    if operation == Operation::Download {
                        assert!(
                            status
                                .conditions
                                .iter()
                                .all(|condition| condition.status == "False")
                        );
                    }
                }
            }
        }
    }
}
