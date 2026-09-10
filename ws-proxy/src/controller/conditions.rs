use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;

pub fn merge(previous: &[Condition], desired: &mut Vec<Condition>) {
    let mut merged: Vec<Condition> = Vec::with_capacity(desired.len());
    for mut condition in desired.drain(..) {
        // Transition time MUST reflect a status change, not a reason or generation change.
        if let Some(old) = previous
            .iter()
            .find(|old| old.type_ == condition.type_ && old.status == condition.status)
        {
            condition.last_transition_time = old.last_transition_time.clone();
        }
        if let Some(current) = merged
            .iter_mut()
            .find(|current| current.type_ == condition.type_)
        {
            *current = condition;
        } else {
            merged.push(condition);
        }
    }
    *desired = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    fn condition(status: &str, generation: i64, time: &str) -> Condition {
        Condition {
            type_: "Updated".into(),
            status: status.into(),
            observed_generation: Some(generation),
            reason: "Reconciling".into(),
            message: "Pending".into(),
            last_transition_time: Time(time.parse().unwrap()),
        }
    }

    #[test]
    fn status_changes_preserve_map_semantics_and_transition_times() {
        let initial = condition("False", 1, "2020-01-01T00:00:00Z");
        let previous = vec![initial.clone()];
        let mut pending = condition("False", 2, "2020-01-02T00:00:00Z");
        pending.reason = "HostOffline".into();
        let mut desired = vec![initial, pending];
        merge(&previous, &mut desired);
        assert_eq!(desired.len(), 1);
        assert_eq!(desired[0].observed_generation, Some(2));
        assert_eq!(
            desired[0].last_transition_time,
            previous[0].last_transition_time
        );
        assert_eq!(desired[0].reason, "HostOffline");

        let applied = condition("True", 2, "2020-01-03T00:00:00Z");
        let previous = desired.clone();
        desired.push(applied.clone());
        merge(&previous, &mut desired);
        assert_eq!(desired, vec![applied]);
        let previous = desired.clone();
        desired.push(condition("True", 2, "2020-01-04T00:00:00Z"));
        merge(&previous, &mut desired);
        assert_eq!(desired, previous);
    }
}
