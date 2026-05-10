use super::*;
use serde_json::json;

mod assistant_blocks {
    use super::*;
    include!("streaming_responses/assistant_blocks.rs");
}

mod basic_errors {
    use super::*;
    include!("streaming_responses/basic_errors.rs");
}

mod images_tools {
    use super::*;
    include!("streaming_responses/images_tools.rs");
}

mod tool_lifecycle {
    use super::*;
    include!("streaming_responses/tool_lifecycle.rs");
}

mod reasoning {
    use super::*;
    include!("streaming_responses/reasoning.rs");
}

mod completed_state {
    use super::*;
    include!("streaming_responses/completed_state.rs");
}
