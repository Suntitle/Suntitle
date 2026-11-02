//! Select one dispatch plan from the current node revision.
//!
//! Deletion selects removal. Built-in nodes select apply. Extension nodes wait
//! for a current publication, then download and apply. Artifact changes invalidate installation; configuration
//! changes invalidate only apply. Both reconciliation loops
//! and acknowledgement validation MUST use this selection.
use crate::{
    kubernetes::{
        adapter::ResolvedAdapter,
        resources::{DispatchStatus, NodePhase, SuntitleNode, SuntitleNodeSpec},
    },
    registration::{ApplyIdentity, Operation, RegistrationServerMessage},
};

/// Work eligible for dispatch. Absence of a plan means publication is pending.
pub(super) enum Plan {
    /// Transfer the current extension artifact and prepare its execution paths.
    Download {
        /// Published reference selected for this node generation.
        reference: String,
        /// Resolved execution and settings corresponding to this node revision.
        adapter: Box<ResolvedAdapter>,
    },
    /// Launch a prepared artifact using the current resolved command configuration.
    CustomApply {
        /// Published payload that MUST already be installed.
        reference: String,
        /// Current command and settings, independent of payload installation.
        adapter: Box<ResolvedAdapter>,
    },
    /// Apply a built-in runtime specification.
    BuiltinApply,
    /// Clean up host work before releasing the node finalizer.
    Remove,
}

impl Plan {
    /// Select work once from deletion, extension, and publication state.
    pub(super) fn for_node(node: &SuntitleNode) -> Option<Self> {
        if node.metadata.deletion_timestamp.is_some() {
            return Some(Self::Remove);
        }
        if node.spec.extension_ref.is_none() {
            return Some(Self::BuiltinApply);
        }
        let status = node.status.as_ref()?;
        status
            .conditions
            .iter()
            .any(|c| c.type_ == "Installed" && c.observed_generation == node.metadata.generation)
            .then_some(())?;
        let reference = status
            .artifact_reference
            .as_ref()
            .filter(|r| !r.is_empty())?;
        let adapter = Box::new(status.resolved_adapter.clone()?);
        if status
            .conditions
            .iter()
            .any(|c| c.type_ == "Installed" && c.status == "True")
            && status
                .dispatch
                .as_ref()
                .is_none_or(|d| d.artifact_reference.as_ref() == Some(reference))
        {
            Some(Self::CustomApply {
                reference: reference.clone(),
                adapter,
            })
        } else {
            Some(Self::Download {
                reference: reference.clone(),
                adapter,
            })
        }
    }

    /// Return the wire operation represented by this plan.
    pub(super) fn operation(&self) -> Operation {
        match self {
            Self::Download { .. } => Operation::Download,
            Self::BuiltinApply | Self::CustomApply { .. } => Operation::Apply,
            Self::Remove => Operation::Remove,
        }
    }

    /// Return the payload identity required for custom preparation and execution.
    pub(super) fn artifact_reference(&self) -> Option<&str> {
        match self {
            Self::Download { reference, .. } | Self::CustomApply { reference, .. } => {
                Some(reference)
            }
            _ => None,
        }
    }

    /// Check whether persisted intent still describes this node and plan.
    pub(super) fn matches(&self, node: &SuntitleNode, dispatch: &DispatchStatus) -> bool {
        Some(dispatch.generation) == node.metadata.generation
            && dispatch.operation == self.operation()
            && dispatch.artifact_reference.as_deref() == self.artifact_reference()
    }

    /// Indicate whether host deletion and finalizer cleanup are permitted.
    pub(super) fn is_removal(&self) -> bool {
        matches!(self, Self::Remove)
    }

    /// Project dispatch preparation without claiming successful execution.
    pub(super) fn pending_phase(&self) -> NodePhase {
        match self {
            Self::Remove => NodePhase::Terminating,
            _ => NodePhase::Pending,
        }
    }

    /// Keep removal terminating while other operations wait for a host session.
    pub(super) fn offline_phase(&self) -> NodePhase {
        match self {
            Self::Remove => NodePhase::Terminating,
            _ => NodePhase::Disconnected,
        }
    }

    /// Consume the selected plan to build its matching host command.
    pub(super) fn into_message(
        self,
        identity: ApplyIdentity,
        spec: SuntitleNodeSpec,
    ) -> RegistrationServerMessage {
        match self {
            Self::Download { reference, adapter } => RegistrationServerMessage::DownloadArtifact {
                identity,
                reference,
                adapter,
            },
            Self::BuiltinApply => RegistrationServerMessage::Apply {
                identity,
                spec: Box::new(spec),
                custom: None,
            },
            Self::CustomApply { reference, adapter } => RegistrationServerMessage::Apply {
                identity,
                spec: Box::new(spec),
                custom: Some(Box::new(crate::registration::CustomExecution {
                    reference,
                    adapter: *adapter,
                })),
            },
            Self::Remove => RegistrationServerMessage::Remove { identity },
        }
    }
}
