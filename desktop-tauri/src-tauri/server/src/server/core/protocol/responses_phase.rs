use serde_json::Value;

pub(super) fn incomplete_reason(finish: Option<&str>) -> Option<&'static str> {
    let finish = finish?.trim();
    if crate::server::core::upstream::continuation::is_length_finish(finish)
        || finish.eq_ignore_ascii_case("max_length")
    {
        Some("max_output_tokens")
    } else if finish.eq_ignore_ascii_case("content_filter") {
        Some("content_filter")
    } else {
        None
    }
}

pub(super) fn message_phase(
    finish: Option<&str>,
    has_tool_calls: bool,
    completed: bool,
) -> Option<&'static str> {
    if !completed {
        return None;
    }
    let finish = finish?.trim();
    if finish.eq_ignore_ascii_case("stop") {
        Some(if has_tool_calls { "commentary" } else { "final_answer" })
    } else if has_tool_calls
        && (finish.eq_ignore_ascii_case("tool_calls") || finish.eq_ignore_ascii_case("function_call"))
    {
        Some("commentary")
    } else {
        None
    }
}

pub(super) fn response_is_successful(body: &Value) -> bool {
    !body.get("error").is_some_and(super::is_truthy)
        && !body.get(super::FIELD_IS_ERROR).is_some_and(super::is_truthy)
        && body.get("status").is_none_or(|status| {
            status.is_null() || status.as_str() == Some("completed")
        })
}

pub(super) fn apply(item: &mut Value, phase: Option<&str>) {
    let Some(phase) = phase else { return };
    if item.get("phase").is_some_and(|value| !value.is_null()) {
        return;
    }
    if phase == "final_answer"
        && super::content_text(item.get("content").unwrap_or(&Value::Null)).trim().is_empty()
    {
        return;
    }
    if let Some(object) = item.as_object_mut() {
        object.insert("phase".into(), Value::String(phase.into()));
    }
}
