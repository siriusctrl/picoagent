use chrono::Utc;

use super::*;

fn record(seq: u64, role: Role, text: &str) -> TrajectoryMessage {
    TrajectoryMessage {
        message_ref: format!("m{seq}"),
        seq,
        created_at: Utc::now(),
        message: Message::text(role, text),
        pending_input_id: None,
        compaction: None,
    }
}

fn assignment_record(seq: u64, task: &str) -> TrajectoryMessage {
    TrajectoryMessage {
        message_ref: format!("m{seq}"),
        seq,
        created_at: Utc::now(),
        message: Message {
            role: Role::User,
            content: vec![
                MessageContent::RuntimeReminder {
                    text: "<runtime-reminder>general task</runtime-reminder>".to_owned(),
                },
                MessageContent::Text {
                    text: task.to_owned(),
                },
            ],
        },
        pending_input_id: None,
        compaction: None,
    }
}

fn compaction_request_record(seq: u64) -> TrajectoryMessage {
    let mut request = record(seq, Role::User, "compact now");
    request.compaction = Some(CompactionMessage::Request);
    request
}

fn state_record(seq: u64, first_kept_message_ref: &str, summary: &str) -> TrajectoryMessage {
    TrajectoryMessage {
        message_ref: format!("m{seq}"),
        seq,
        created_at: Utc::now(),
        message: Message::text(Role::Assistant, summary),
        pending_input_id: None,
        compaction: Some(CompactionMessage::State {
            state: CompactionState {
                covered_through_message_ref: "m2".to_owned(),
                first_kept_message_ref: first_kept_message_ref.to_owned(),
            },
        }),
    }
}

fn contains_exact_text(message: &Message, expected: &str) -> bool {
    message
        .content
        .iter()
        .any(|content| matches!(content, MessageContent::Text { text } if text == expected))
}

#[test]
fn fork_assignment_in_kept_tail_is_not_duplicated_in_active_or_compaction_context() {
    let task = "inspect only; do not edit";
    let trajectory = vec![
        record(1, Role::User, "root workflow"),
        record(2, Role::Assistant, "old inherited work"),
        record(3, Role::User, "ancestor orchestration"),
        assignment_record(4, task),
        record(5, Role::Assistant, "recent child work"),
        compaction_request_record(6),
        state_record(7, "m4", "state with assignment kept"),
    ];

    let active = build_active_context(&trajectory, Some(3)).unwrap();
    assert_eq!(
        active
            .iter()
            .filter(|message| contains_exact_text(message, task))
            .count(),
        1
    );
    assert_eq!(
        serde_json::to_value(&active[3]).unwrap(),
        serde_json::to_value(&trajectory[3].message).unwrap()
    );

    let plan = CompactionPlan {
        to_compact: vec![&trajectory[1], &trajectory[2]],
        covered_through: &trajectory[2],
        first_kept: &trajectory[3],
    };
    let instruction = Message::text(Role::User, "compact");
    let input = compaction_input(&trajectory, None, &plan, &instruction, Some(3)).unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|message| contains_exact_text(message, task))
            .count(),
        1
    );
    assert_eq!(
        serde_json::to_value(&input[input.len() - 2]).unwrap(),
        serde_json::to_value(&trajectory[3].message).unwrap()
    );

    let assignment_compacted_plan = CompactionPlan {
        to_compact: vec![&trajectory[1], &trajectory[2], &trajectory[3]],
        covered_through: &trajectory[3],
        first_kept: &trajectory[4],
    };
    let input = compaction_input(
        &trajectory,
        None,
        &assignment_compacted_plan,
        &instruction,
        Some(3),
    )
    .unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|message| contains_exact_text(message, task))
            .count(),
        1
    );
}

#[test]
fn repeated_nested_fork_compaction_pins_only_the_current_assignment() {
    let ancestor_task = "ancestor task must edit";
    let current_task = "nested inspection only";
    let first_state = state_record(6, "m5", "first compacted state");
    let mut trajectory = vec![
        record(1, Role::User, "root workflow"),
        assignment_record(2, ancestor_task),
        record(3, Role::Assistant, "ancestor work"),
        assignment_record(4, current_task),
        record(5, Role::Assistant, "older child work"),
        first_state.clone(),
        record(7, Role::User, "work after first state"),
        record(8, Role::Assistant, "recent nested work"),
    ];
    let plan = CompactionPlan {
        to_compact: vec![&trajectory[6]],
        covered_through: &trajectory[6],
        first_kept: &trajectory[7],
    };
    let instruction = Message::text(Role::User, "compact again");
    let input = compaction_input(
        &trajectory,
        Some((&first_state, first_state.compaction_state().unwrap())),
        &plan,
        &instruction,
        Some(3),
    )
    .unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|message| contains_exact_text(message, current_task))
            .count(),
        1
    );
    assert!(
        !input
            .iter()
            .any(|message| contains_exact_text(message, ancestor_task))
    );
    assert_eq!(input[1].visible_text(), first_state.message.visible_text());
    assert!(contains_exact_text(&input[2], current_task));

    trajectory.push(compaction_request_record(9));
    trajectory.push(state_record(10, "m8", "second compacted state"));
    let active = build_active_context(&trajectory, Some(3)).unwrap();
    assert_eq!(
        active
            .iter()
            .filter(|message| contains_exact_text(message, current_task))
            .count(),
        1
    );
    assert!(
        !active
            .iter()
            .any(|message| contains_exact_text(message, ancestor_task))
    );
    assert!(contains_exact_text(&active[3], current_task));
    assert_eq!(active[4].visible_text(), "recent nested work");
}
