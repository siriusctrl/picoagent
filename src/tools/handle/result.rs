use serde_json::{Value, json};

use crate::agent::handle::HandleSnapshot;

pub(super) fn handle_snapshots(handles: Vec<HandleSnapshot>) -> Value {
    json!({ "handles": handles })
}

pub(super) fn handle_snapshot(handle: HandleSnapshot) -> Value {
    json!(handle)
}
