use super::types::{MonoizeChannel, MonoizeModelEntry, MonoizeProvider};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

/// DPT-RC2: enabled providers in routing order, each reduced to its enabled
/// channels with positive weight.
pub(super) struct RoutingCatalog {
    providers: Vec<CatalogProvider>,
    /// Sorted, distinct model names served by `providers`.
    pub(super) model_names: Vec<String>,
    providers_by_model: DashMap<String, Arc<[MonoizeProvider]>>,
}

/// Templates hold empty `channels`/`models` so per-model projections clone
/// only the rows they return.
struct CatalogProvider {
    template: MonoizeProvider,
    channels: Vec<CatalogChannel>,
}

struct CatalogChannel {
    template: MonoizeChannel,
    models: HashMap<String, MonoizeModelEntry>,
}

impl RoutingCatalog {
    pub(super) fn build(providers: Vec<MonoizeProvider>) -> Self {
        let providers = providers
            .into_iter()
            .filter(|provider| provider.enabled)
            .map(|mut template| {
                let channels = std::mem::take(&mut template.channels)
                    .into_iter()
                    .filter(|channel| channel.enabled && channel.weight > 0)
                    .map(|mut template| CatalogChannel {
                        models: std::mem::take(&mut template.models),
                        template,
                    })
                    .collect();
                CatalogProvider { template, channels }
            })
            .collect::<Vec<_>>();
        let mut model_names = providers
            .iter()
            .flat_map(|provider| &provider.channels)
            .flat_map(|channel| channel.models.keys().cloned())
            .collect::<Vec<_>>();
        model_names.sort_unstable();
        model_names.dedup();
        Self {
            providers,
            model_names,
            providers_by_model: DashMap::new(),
        }
    }

    pub(super) fn contains_model(&self, model: &str) -> bool {
        self.model_names
            .binary_search_by(|name| name.as_str().cmp(model))
            .is_ok()
    }

    pub(super) fn providers_for_model(&self, model: &str) -> Arc<[MonoizeProvider]> {
        if !self.contains_model(model) {
            return Arc::from([]);
        }
        if let Some(cached) = self.providers_by_model.get(model) {
            return Arc::clone(&cached);
        }
        let projected = self
            .providers
            .iter()
            .filter_map(|provider| {
                let channels = provider
                    .channels
                    .iter()
                    .filter_map(|channel| {
                        let entry = channel.models.get(model)?;
                        let mut projected = channel.template.clone();
                        projected.models = HashMap::from([(model.to_string(), entry.clone())]);
                        Some(projected)
                    })
                    .collect::<Vec<_>>();
                (!channels.is_empty()).then(|| MonoizeProvider {
                    channels,
                    ..provider.template.clone()
                })
            })
            .collect::<Arc<[_]>>();
        Arc::clone(
            &self
                .providers_by_model
                .entry(model.to_string())
                .or_insert(projected),
        )
    }
}
