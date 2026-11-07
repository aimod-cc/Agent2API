use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::server::core::debug_traffic::TrafficCapture;
use crate::server::core::upstream::usage::RequestTelemetry;

fn captures() -> &'static Mutex<HashMap<String, Weak<TrafficCapture>>> {
    static CAPTURES: OnceLock<Mutex<HashMap<String, Weak<TrafficCapture>>>> = OnceLock::new();
    CAPTURES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register(telemetry: &RequestTelemetry) -> Option<String> {
    let capture = telemetry.capture()?;
    let mut captures = captures().lock().ok()?;
    captures.retain(|_, capture| capture.strong_count() > 0);
    if captures.len() >= 1024 {
        return None;
    }
    let id = crate::server::core::protocol::random_id("script-trace");
    captures.insert(id.clone(), Arc::downgrade(&capture));
    Some(id)
}

pub fn find(id: &str) -> Option<Arc<TrafficCapture>> {
    captures().lock().ok()?.get(id)?.upgrade()
}
