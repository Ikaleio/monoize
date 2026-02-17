use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct VectorPoint {
    pub tenant_id: String,
    pub vector_store_id: String,
    pub file_id: String,
    pub chunk_id: String,
    pub vector: Vec<f32>,
    pub text: String,
    pub attributes: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_id: String,
    pub chunk_id: String,
    pub score: f32,
    pub text: String,
    pub attributes: Option<Value>,
}

#[async_trait]
pub trait VectorIndex: Send + Sync {
    async fn upsert(&self, points: Vec<VectorPoint>) -> Result<(), String>;
    async fn search(
        &self,
        tenant_id: &str,
        vector_store_id: &str,
        query_vector: &[f32],
        filter: Option<&Value>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>, String>;
    async fn delete_by_file(&self, tenant_id: &str, file_id: &str) -> Result<(), String>;
    async fn delete_by_vector_store(
        &self,
        tenant_id: &str,
        vector_store_id: &str,
    ) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct MemoryVectorIndex {
    inner: Arc<RwLock<Vec<VectorPoint>>>,
}

#[async_trait]
impl VectorIndex for MemoryVectorIndex {
    async fn upsert(&self, points: Vec<VectorPoint>) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        for point in points {
            if let Some(existing) = guard.iter_mut().find(|item| {
                item.tenant_id == point.tenant_id
                    && item.vector_store_id == point.vector_store_id
                    && item.file_id == point.file_id
                    && item.chunk_id == point.chunk_id
            }) {
                *existing = point;
            } else {
                guard.push(point);
            }
        }
        Ok(())
    }

    async fn search(
        &self,
        tenant_id: &str,
        vector_store_id: &str,
        query_vector: &[f32],
        filter: Option<&Value>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>, String> {
        let guard = self.inner.read().await;
        let mut results = Vec::new();
        for point in guard.iter() {
            if point.tenant_id != tenant_id || point.vector_store_id != vector_store_id {
                continue;
            }
            if let Some(filter_value) = filter {
                if !matches_filter(point.attributes.as_ref(), filter_value) {
                    continue;
                }
            }
            let score = cosine_similarity(query_vector, &point.vector);
            results.push(SearchResult {
                file_id: point.file_id.clone(),
                chunk_id: point.chunk_id.clone(),
                score,
                text: point.text.clone(),
                attributes: point.attributes.clone(),
            });
        }
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        Ok(results)
    }

    async fn delete_by_file(&self, tenant_id: &str, file_id: &str) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        guard.retain(|point| !(point.tenant_id == tenant_id && point.file_id == file_id));
        Ok(())
    }

    async fn delete_by_vector_store(
        &self,
        tenant_id: &str,
        vector_store_id: &str,
    ) -> Result<(), String> {
        let mut guard = self.inner.write().await;
        guard.retain(|point| {
            !(point.tenant_id == tenant_id && point.vector_store_id == vector_store_id)
        });
        Ok(())
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

fn matches_filter(attributes: Option<&Value>, filter: &Value) -> bool {
    let Some(attributes) = attributes else {
        return false;
    };
    let Some(filter_obj) = filter.as_object() else {
        return true;
    };
    let Some(attr_obj) = attributes.as_object() else {
        return false;
    };
    for (key, expected) in filter_obj {
        if attr_obj.get(key) != Some(expected) {
            return false;
        }
    }
    true
}
