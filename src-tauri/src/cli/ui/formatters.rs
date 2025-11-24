use serde::Serialize;

pub fn to_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(value)
}

pub fn format_bool(value: bool) -> &'static str {
    if value {
        "✓"
    } else {
        "✗"
    }
}
