use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRecord {
    pub logical_model: String,
    pub provider_id: String,
    pub upstream_model: String,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilities {
    pub max_context_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_structured_output: bool,
    pub supports_reasoning_controls: ReasoningControls,
    pub supports_image_input: ImageInputSupport,
    pub supports_file_input: FileInputSupport,
    pub supports_image_output: ImageOutputSupport,
    pub tokenizer: Option<String>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            max_context_tokens: None,
            max_output_tokens: None,
            supports_streaming: true,
            supports_tools: true,
            supports_parallel_tool_calls: true,
            supports_structured_output: false,
            supports_reasoning_controls: ReasoningControls::default(),
            supports_image_input: ImageInputSupport::default(),
            supports_file_input: FileInputSupport::default(),
            supports_image_output: ImageOutputSupport::default(),
            tokenizer: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningControls {
    pub supported: bool,
    pub mode: String,
    pub effort_levels: Vec<String>,
    pub max_reasoning_tokens: Option<u64>,
}

impl Default for ReasoningControls {
    fn default() -> Self {
        Self {
            supported: false,
            mode: "none".to_string(),
            effort_levels: Vec::new(),
            max_reasoning_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageInputSupport {
    pub supported: bool,
    pub max_images: Option<u64>,
}

impl Default for ImageInputSupport {
    fn default() -> Self {
        Self {
            supported: false,
            max_images: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileInputSupport {
    pub supported: bool,
    pub max_files: Option<u64>,
}

impl Default for FileInputSupport {
    fn default() -> Self {
        Self {
            supported: false,
            max_files: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageOutputSupport {
    pub supported: bool,
}

impl Default for ImageOutputSupport {
    fn default() -> Self {
        Self { supported: false }
    }
}

#[derive(Clone)]
pub struct ModelRegistry {
    inner: Arc<RwLock<HashMap<(String, String), ModelRecord>>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn all_records(&self) -> Vec<ModelRecord> {
        let guard = self.inner.read().await;
        guard.values().cloned().collect()
    }

    pub async fn find_candidates(&self, logical_model: &str) -> Vec<ModelRecord> {
        let guard = self.inner.read().await;
        guard
            .values()
            .filter(|record| record.logical_model == logical_model)
            .cloned()
            .collect()
    }

    /// Replace the in-memory registry with enabled database records.
    pub async fn replace_db_records(
        &self,
        db_records: Vec<crate::model_registry_store::DbModelRecord>,
    ) {
        let mut guard = self.inner.write().await;
        guard.clear();
        for db_record in db_records {
            let record = db_record.to_model_record();
            guard.insert(
                (record.logical_model.clone(), record.provider_id.clone()),
                record,
            );
        }
    }

    /// Merge database records into the registry.
    pub async fn merge_db_records(
        &self,
        db_records: Vec<crate::model_registry_store::DbModelRecord>,
    ) {
        let mut guard = self.inner.write().await;
        for db_record in db_records {
            let record = db_record.to_model_record();
            guard.insert(
                (record.logical_model.clone(), record.provider_id.clone()),
                record,
            );
        }
    }
}
