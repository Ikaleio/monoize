#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseZone {
    Empty,
    InReasoning,
    InContent,
    InAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Reasoning,
    Content,
    Action,
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
