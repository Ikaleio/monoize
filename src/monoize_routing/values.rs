use sea_orm::Value as SeaValue;

pub(super) fn opt_bool_to_value(v: Option<bool>) -> SeaValue {
    match v {
        Some(b) => SeaValue::Int(Some(if b { 1 } else { 0 })),
        None => SeaValue::Int(None),
    }
}

pub(super) fn opt_u64_to_value(v: Option<u64>) -> SeaValue {
    match v {
        Some(n) => SeaValue::Int(Some(n as i32)),
        None => SeaValue::Int(None),
    }
}
