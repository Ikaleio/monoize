use super::*;
use base64::Engine as _;

fn last_captured_body(ctx: &TestContext, endpoint: &str) -> Value {
    ctx.captured_bodies
        .lock()
        .expect("captured bodies lock")
        .iter()
        .rev()
        .find(|(name, _)| name == endpoint)
        .map(|(_, body)| body.clone())
        .unwrap_or_else(|| panic!("missing captured upstream body for {endpoint}"))
}

mod responses_reasoning {
    use super::*;
    include!("adapters_nonstream/responses_reasoning.rs");
}

mod images_and_chat {
    use super::*;
    include!("adapters_nonstream/images_and_chat.rs");
}

mod messages_basic {
    use super::*;
    include!("adapters_nonstream/messages_basic.rs");
}

mod tools_envelope {
    use super::*;
    include!("adapters_nonstream/tools_envelope.rs");
}

mod reasoning_tools {
    use super::*;
    include!("adapters_nonstream/reasoning_tools.rs");
}

mod messages_native {
    use super::*;
    include!("adapters_nonstream/messages_native.rs");
}

mod native_responses {
    use super::*;
    include!("adapters_nonstream/native_responses.rs");
}

mod messages_reasoning {
    use super::*;
    include!("adapters_nonstream/messages_reasoning.rs");
}
