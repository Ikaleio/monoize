//! PR5b: canonical events follow upstream `output_index` order.

use crate::urp::UrpStreamEvent;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Holds the events of every item behind a lower item that is still open, and numbers raw node
/// indices in release order, so canonical `node_index` order follows `output_index` order.
#[derive(Default)]
pub(super) struct OutputOrderGate {
    open: BTreeSet<u64>,
    held: BTreeMap<u64, Vec<UrpStreamEvent>>,
    canonical: HashMap<u32, u32>,
}

/// One item released by [`OutputOrderGate::drain`].
pub(super) struct Released {
    pub(super) output_index: u64,
    pub(super) events: Vec<UrpStreamEvent>,
    /// The item is still open while a later released item follows it, so the caller closes it
    /// before that item starts and no node joins another item.
    pub(super) close_before_next: bool,
}

impl OutputOrderGate {
    pub(super) fn announce(&mut self, output_index: u64) {
        self.open.insert(output_index);
    }

    pub(super) fn blocked(&self, output_index: u64) -> bool {
        self.open.range(..output_index).next().is_some()
    }

    /// Holds `events`; an empty list still marks the item as released later.
    pub(super) fn hold(&mut self, output_index: u64, events: Vec<UrpStreamEvent>) {
        self.held.entry(output_index).or_default().extend(events);
    }

    /// Closes one item and returns the held events it no longer blocks.
    pub(super) fn complete(&mut self, output_index: u64) -> Vec<UrpStreamEvent> {
        self.open.remove(&output_index);
        let mut released = Vec::new();
        while let Some(entry) = self.held.first_entry() {
            if self.open.range(..*entry.key()).next().is_some() {
                break;
            }
            released.extend(entry.remove());
        }
        released
    }

    /// Releases every held item in `output_index` order and ends the gate. Held items absent
    /// from `retained` (the items an authoritative terminal kept) are dropped.
    pub(super) fn drain(&mut self, retained: Option<&BTreeSet<u64>>) -> Vec<Released> {
        let open = std::mem::take(&mut self.open);
        let mut held = std::mem::take(&mut self.held);
        if let Some(retained) = retained {
            held.retain(|output_index, _| retained.contains(output_index));
        }
        let indices: BTreeSet<u64> = open.iter().chain(held.keys()).copied().collect();
        indices
            .into_iter()
            .map(|output_index| Released {
                output_index,
                close_before_next: open.contains(&output_index)
                    && held.range(output_index + 1..).next().is_some(),
                events: held.remove(&output_index).unwrap_or_default(),
            })
            .collect()
    }

    /// Replaces raw node indices with canonical ones. A node gets the next canonical index
    /// when its first event is released; a terminal entry keeps its key only when its node
    /// had events downstream (STR-8a).
    pub(super) fn canonicalize(&mut self, event: &mut UrpStreamEvent) {
        match event {
            UrpStreamEvent::NodeStart { node_index, .. }
            | UrpStreamEvent::NodeDelta { node_index, .. }
            | UrpStreamEvent::NodeDone { node_index, .. } => {
                let next = self.canonical.len() as u32;
                *node_index = *self.canonical.entry(*node_index).or_insert(next);
            }
            UrpStreamEvent::ResponseDone { output, .. } => {
                for entry in output {
                    entry.node_index = entry
                        .node_index
                        .and_then(|raw| self.canonical.get(&raw).copied());
                }
            }
            _ => {}
        }
    }
}
