use super::{Part, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseZone {
    Empty,
    InReasoning,
    InContent,
    InAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Append,
    FlushAndNew(Vec<Part>),
}

#[derive(Debug, Clone)]
pub struct GreedyMerger {
    zone: PhaseZone,
    current_role: Option<Role>,
    pending_parts: Vec<Part>,
}

impl GreedyMerger {
    pub fn new() -> Self {
        Self {
            zone: PhaseZone::Empty,
            current_role: None,
            pending_parts: Vec::new(),
        }
    }

    pub fn feed(&mut self, part: Part, role: Role) -> Action {
        if self.current_role != Some(role) && !self.pending_parts.is_empty() {
            let flushed = std::mem::take(&mut self.pending_parts);
            self.current_role = Some(role);
            self.zone = Self::zone_for_part(&part);
            self.pending_parts.push(part);
            return Action::FlushAndNew(flushed);
        }

        self.current_role = Some(role);

        match Self::kind_for_part(&part) {
            PartKind::Reasoning => {
                if matches!(self.zone, PhaseZone::InContent | PhaseZone::InAction) {
                    let flushed = std::mem::take(&mut self.pending_parts);
                    self.zone = PhaseZone::InReasoning;
                    self.pending_parts.push(part);
                    return Action::FlushAndNew(flushed);
                }
                self.zone = PhaseZone::InReasoning;
            }
            PartKind::Content => {
                if matches!(self.zone, PhaseZone::InContent | PhaseZone::InAction) {
                    let flushed = std::mem::take(&mut self.pending_parts);
                    self.zone = PhaseZone::InContent;
                    self.pending_parts.push(part);
                    return Action::FlushAndNew(flushed);
                }
                self.zone = PhaseZone::InContent;
            }
            PartKind::Action => {
                self.zone = PhaseZone::InAction;
            }
        }

        self.pending_parts.push(part);
        Action::Append
    }

    pub fn finish(&mut self) -> Option<Vec<Part>> {
        if self.pending_parts.is_empty() {
            self.zone = PhaseZone::Empty;
            self.current_role = None;
            return None;
        }

        let flushed = std::mem::take(&mut self.pending_parts);
        self.zone = PhaseZone::Empty;
        self.current_role = None;
        Some(flushed)
    }

    fn kind_for_part(part: &Part) -> PartKind {
        match part {
            Part::Reasoning { .. } => PartKind::Reasoning,
            Part::Text { .. }
            | Part::Image { .. }
            | Part::Audio { .. }
            | Part::File { .. }
            | Part::Refusal { .. } => PartKind::Content,
            Part::ToolCall { .. } | Part::ProviderItem { .. } => PartKind::Action,
        }
    }

    fn zone_for_part(part: &Part) -> PhaseZone {
        match Self::kind_for_part(part) {
            PartKind::Reasoning => PhaseZone::InReasoning,
            PartKind::Content => PhaseZone::InContent,
            PartKind::Action => PhaseZone::InAction,
        }
    }
}

impl Default for GreedyMerger {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Reasoning,
    Content,
    Action,
}

#[cfg(test)]
mod tests {
    use super::{Action, GreedyMerger};
    use crate::urp::{Part, Role};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn sequential_reasoning_parts_do_not_flush() {
        let mut merger = GreedyMerger::new();

        assert_eq!(
            merger.feed(reasoning("r1"), Role::Assistant),
            Action::Append
        );
        assert_eq!(
            merger.feed(reasoning("r2"), Role::Assistant),
            Action::Append
        );
        assert_eq!(
            merger.finish(),
            Some(vec![reasoning("r1"), reasoning("r2")])
        );
    }

    #[test]
    fn reasoning_then_text_does_not_flush() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(reasoning("r"), Role::Assistant), Action::Append);
        assert_eq!(merger.feed(text("t"), Role::Assistant), Action::Append);
        assert_eq!(merger.finish(), Some(vec![reasoning("r"), text("t")]));
    }

    #[test]
    fn text_then_tool_call_does_not_flush() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(text("t"), Role::Assistant), Action::Append);
        assert_eq!(merger.feed(tool_call("1"), Role::Assistant), Action::Append);
        assert_eq!(merger.finish(), Some(vec![text("t"), tool_call("1")]));
    }

    #[test]
    fn tool_call_then_text_flushes() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(tool_call("1"), Role::Assistant), Action::Append);
        assert_flushes_to(
            merger.feed(text("t"), Role::Assistant),
            vec![tool_call("1")],
        );
        assert_eq!(merger.finish(), Some(vec![text("t")]));
    }

    #[test]
    fn tool_call_then_reasoning_flushes() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(tool_call("1"), Role::Assistant), Action::Append);
        assert_flushes_to(
            merger.feed(reasoning("r"), Role::Assistant),
            vec![tool_call("1")],
        );
        assert_eq!(merger.finish(), Some(vec![reasoning("r")]));
    }

    #[test]
    fn content_then_reasoning_flushes() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(text("t"), Role::Assistant), Action::Append);
        assert_flushes_to(
            merger.feed(reasoning("r"), Role::Assistant),
            vec![text("t")],
        );
        assert_eq!(merger.finish(), Some(vec![reasoning("r")]));
    }

    #[test]
    fn role_change_flushes() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(text("t"), Role::User), Action::Append);
        assert_flushes_to(merger.feed(text("u"), Role::Assistant), vec![text("t")]);
        assert_eq!(merger.finish(), Some(vec![text("u")]));
    }

    #[test]
    fn multiple_tool_calls_do_not_flush() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.feed(tool_call("1"), Role::Assistant), Action::Append);
        assert_eq!(merger.feed(tool_call("2"), Role::Assistant), Action::Append);
        assert_eq!(merger.finish(), Some(vec![tool_call("1"), tool_call("2")]));
    }

    #[test]
    fn empty_finish_returns_none() {
        let mut merger = GreedyMerger::new();

        assert_eq!(merger.finish(), None);
    }

    #[test]
    fn finish_with_pending_returns_parts() {
        let mut merger = GreedyMerger::new();

        assert_eq!(
            merger.feed(provider_item(), Role::Assistant),
            Action::Append
        );
        assert_eq!(merger.finish(), Some(vec![provider_item()]));
    }

    #[test]
    fn text_then_text_flushes() {
        let mut merger = GreedyMerger::new();
        assert_eq!(merger.feed(text("a"), Role::Assistant), Action::Append);
        assert_flushes_to(merger.feed(text("b"), Role::Assistant), vec![text("a")]);
        assert_eq!(merger.finish(), Some(vec![text("b")]));
    }

    fn text(content: &str) -> Part {
        Part::Text {
            content: content.to_owned(),
            extra_body: HashMap::new(),
        }
    }

    fn reasoning(content: &str) -> Part {
        Part::Reasoning { id: None, content: Some(content.to_owned()), encrypted: None, summary: None, source: None, extra_body: HashMap::new() }
    }

    fn tool_call(call_id: &str) -> Part {
        Part::ToolCall { id: None, call_id: call_id.to_owned(), name: "lookup".to_owned(), arguments: "{}".to_owned(), extra_body: HashMap::new() }
    }

    fn provider_item() -> Part {
        Part::ProviderItem { id: None, item_type: "raw".to_owned(), body: json!({"ok": true}), extra_body: HashMap::new() }
    }

    fn assert_flushes_to(action: Action, expected: Vec<Part>) {
        match action {
            Action::FlushAndNew(parts) => assert_eq!(parts, expected),
            Action::Append => panic!("expected flush action"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeAction {
    Append,
    FlushAndNew(Vec<super::Node>),
}

#[derive(Debug, Clone)]
pub struct NodeGreedyMerger {
    zone: PhaseZone,
    current_role: Option<super::OrdinaryRole>,
    pending: Vec<super::Node>,
}

impl NodeGreedyMerger {
    pub fn new() -> Self {
        Self {
            zone: PhaseZone::Empty,
            current_role: None,
            pending: Vec::new(),
        }
    }

    pub fn feed(&mut self, node: super::Node) -> NodeAction {
        let role = node.role();
        if role.is_some()
            && self.current_role.is_some()
            && self.current_role != role
            && !self.pending.is_empty()
        {
            let flushed = std::mem::take(&mut self.pending);
            self.current_role = role;
            self.zone = Self::zone_for(&node);
            self.pending.push(node);
            return NodeAction::FlushAndNew(flushed);
        }
        if role.is_some() {
            self.current_role = role;
        }

        match Self::kind(&node) {
            PartKind::Reasoning => {
                if matches!(self.zone, PhaseZone::InContent | PhaseZone::InAction) {
                    let flushed = std::mem::take(&mut self.pending);
                    self.zone = PhaseZone::InReasoning;
                    self.pending.push(node);
                    return NodeAction::FlushAndNew(flushed);
                }
                self.zone = PhaseZone::InReasoning;
            }
            PartKind::Content => {
                if matches!(self.zone, PhaseZone::InContent | PhaseZone::InAction) {
                    let flushed = std::mem::take(&mut self.pending);
                    self.zone = PhaseZone::InContent;
                    self.pending.push(node);
                    return NodeAction::FlushAndNew(flushed);
                }
                self.zone = PhaseZone::InContent;
            }
            PartKind::Action => {
                self.zone = PhaseZone::InAction;
            }
        }

        self.pending.push(node);
        NodeAction::Append
    }

    pub fn finish(&mut self) -> Option<Vec<super::Node>> {
        if self.pending.is_empty() {
            self.zone = PhaseZone::Empty;
            self.current_role = None;
            return None;
        }
        let flushed = std::mem::take(&mut self.pending);
        self.zone = PhaseZone::Empty;
        self.current_role = None;
        Some(flushed)
    }

    fn kind(node: &super::Node) -> PartKind {
        match node {
            super::Node::Reasoning { .. } => PartKind::Reasoning,
            super::Node::Text { .. }
            | super::Node::Image { .. }
            | super::Node::Audio { .. }
            | super::Node::File { .. }
            | super::Node::Refusal { .. } => PartKind::Content,
            super::Node::ToolCall { .. }
            | super::Node::ProviderItem { .. }
            | super::Node::ToolResult { .. }
            | super::Node::NextDownstreamEnvelopeExtra { .. } => PartKind::Action,
        }
    }

    fn zone_for(node: &super::Node) -> PhaseZone {
        match Self::kind(node) {
            PartKind::Reasoning => PhaseZone::InReasoning,
            PartKind::Content => PhaseZone::InContent,
            PartKind::Action => PhaseZone::InAction,
        }
    }
}

impl Default for NodeGreedyMerger {
    fn default() -> Self {
        Self::new()
    }
}
