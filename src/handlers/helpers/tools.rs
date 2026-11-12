use super::*;

mod filtering;
mod namespace;
mod promotion;

use filtering::selector_name;
use namespace::bridge_namespace_selector;
use promotion::active_tool_transports;

pub(in crate::handlers) use filtering::{
    filter_tools_for_provider, map_web_search_tools_for_provider,
};
pub(in crate::handlers) use namespace::{
    restore_messages_custom_tool_calls, restore_tool_namespace_event, restore_tool_namespace_node,
    tool_namespace_aliases,
};
pub(super) use promotion::messages_custom_bridge_names;
pub(in crate::handlers) use promotion::promote_responses_additional_tools;

pub(in crate::handlers) fn server_tool_usage_classes(
    tools: Option<&[urp::ToolDefinition]>,
) -> Vec<String> {
    let Some(tools) = tools else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for tool in tools {
        let Some(class) = server_tool_usage_class(tool.tool_type.as_str()) else {
            continue;
        };
        if !out.iter().any(|existing| existing == class) {
            out.push(class.to_string());
        }
    }
    out
}

fn server_tool_usage_class(tool_type: &str) -> Option<&'static str> {
    match tool_type {
        "web_search" | "web_search_preview" | "web_fetch" => Some("web_search"),
        "file_search" | "collections_search" | "attachment_search" => Some("file_search_tool_call"),
        "x_search" => Some("x_search"),
        "code_interpreter" => Some("code_interpreter_duration"),
        "code_execution" => Some("code_execution_duration"),
        _ => None,
    }
}

#[cfg(test)]
use promotion::tool_wire_name;
#[cfg(test)]
#[path = "../tool_namespace_tests.rs"]
mod tool_namespace_tests;
