//! Tool-call argument events (PR8a: function and custom calls share one lifecycle).

use super::item::CallAccumulator;
use super::items::output_index;
use super::nodes::wire_extra;
use super::state::ResponsesStreamDecoder;
use crate::urp::{NodeDelta, ToolCallType, UrpStreamEvent};
use serde_json::Value;

fn is_custom(event_name: &str) -> bool {
    event_name.starts_with("response.custom_tool_call_input.")
}

impl ResponsesStreamDecoder {
    pub(super) fn on_arguments_delta(
        &mut self,
        event_name: &str,
        data: &Value,
    ) -> Vec<UrpStreamEvent> {
        let custom = is_custom(event_name);
        let delta = data
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = data.get("name").and_then(Value::as_str).unwrap_or_default();
        let output_index = output_index(data);
        let item = self.item(output_index);
        item.arguments_delta_seen = true;
        if item.call.is_none()
            && let Some(call_id) = data.get("call_id").and_then(Value::as_str)
        {
            let tool_type = if custom {
                ToolCallType::Custom
            } else {
                ToolCallType::Function
            };
            item.call = Some(CallAccumulator::new(call_id, tool_type, name, ""));
        }
        if let Some(call) = item.call.as_mut() {
            call.apply_delta(custom, name, delta);
        }
        vec![UrpStreamEvent::NodeDelta {
            node_index: self.item_node_index(output_index),
            delta: NodeDelta::ToolCallArguments {
                arguments: delta.to_string(),
            },
            usage: None,
            extra_body: wire_extra(
                data,
                &[
                    "type",
                    "sequence_number",
                    "delta",
                    "output_index",
                    "content_index",
                    "part_index",
                ],
            ),
        }]
    }

    /// A non-empty done snapshot is the complete argument string (PR6c).
    pub(super) fn on_arguments_done(&mut self, event_name: &str, data: &Value) {
        let custom = is_custom(event_name);
        let arguments = data
            .get(if custom { "input" } else { "arguments" })
            .or_else(|| data.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(call) = self.item(output_index(data)).call.as_mut() {
            call.apply_snapshot(custom, "", arguments);
        }
    }
}
