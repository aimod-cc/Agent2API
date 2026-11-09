use serde_json::{json, Value};

mod provider_docs;

pub fn snapshot() -> Value {
    let host = super::runtime::status().map(|status| {
        json!({
            "running": status.running, "ready": status.ready, "providers": status.providers, "crashes": status.crashes,
            "disabled": status.disabled, "lastError": status.last_error,
        })
    });
    let packs = super::list_packs();
    let provider_docs = provider_docs::snapshot(&packs);
    json!({
        "packs": packs, "host": host, "wasm": super::wasm::contract(),
        "adminPanels": super::admin_panels(), "accountFeatures": super::account_features(),
        "providerDocs": provider_docs,
    })
}
