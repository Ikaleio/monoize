pub fn synthetic_message_id() -> String {
    format!("msg_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_reasoning_id() -> String {
    format!("rs_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_tool_call_id() -> String {
    format!("fc_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_tool_result_id() -> String {
    format!("fco_urp_{}", uuid::Uuid::new_v4().simple())
}

pub fn synthetic_provider_item_id() -> String {
    format!("pi_urp_{}", uuid::Uuid::new_v4().simple())
}
