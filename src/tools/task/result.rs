use serde_json::{Value, json};

use crate::agent::task::BackgroundTaskRecord;

pub(super) fn task_records(records: Vec<BackgroundTaskRecord>) -> Value {
    let tasks = records.into_iter().map(task_record).collect::<Vec<_>>();
    json!({ "tasks": tasks })
}

pub(super) fn task_record(record: BackgroundTaskRecord) -> Value {
    json!({
        "task_id": record.id,
        "kind": record.kind,
        "name": record.name,
        "status": record.status(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_structured_without_explanatory_messages() {
        let record = BackgroundTaskRecord::queued_tool(
            "t1".to_owned(),
            "bash".to_owned(),
            "call-1".to_owned(),
        );

        assert_eq!(
            task_records(vec![record]),
            json!({
                "tasks": [{
                    "task_id": "t1",
                    "kind": "tool",
                    "name": "bash",
                    "status": "queued"
                }]
            })
        );
    }

    #[test]
    fn one_task_is_not_wrapped_in_a_collection() {
        let record = BackgroundTaskRecord::queued_tool(
            "t1".to_owned(),
            "bash".to_owned(),
            "call-1".to_owned(),
        );

        assert_eq!(
            task_record(record),
            json!({
                "task_id": "t1",
                "kind": "tool",
                "name": "bash",
                "status": "queued"
            })
        );
    }
}
