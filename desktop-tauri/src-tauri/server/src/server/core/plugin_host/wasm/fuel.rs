use serde_json::{json, Value};

pub(super) const BASE_FUEL: u64 = 2_000_000_000;
pub(super) const MAX_INPUT_FUEL: u64 = 8_000_000_000;
pub(super) const INPUT_FUEL_PER_BYTE: u64 = 2048;
pub(super) const YIELD_INTERVAL: u64 = 100_000;

pub(super) struct Budget {
    adaptive: bool,
    base: u64,
    granted: u64,
    window: u64,
}

impl Budget {
    pub(super) fn new(adaptive: bool, verification: bool) -> Self {
        let adaptive = adaptive && !verification;
        let base = if verification { super::verification::FUEL } else { BASE_FUEL };
        Self { adaptive, base, granted: 0, window: base }
    }

    pub(super) fn is_adaptive(&self) -> bool {
        self.adaptive
    }

    pub(super) fn input(&mut self, bytes: usize) -> u64 {
        self.granted = if self.adaptive {
            self.base.saturating_add((bytes as u64).saturating_mul(INPUT_FUEL_PER_BYTE)).min(MAX_INPUT_FUEL)
        } else { self.base };
        self.window = self.granted;
        self.granted
    }

    // 只要上游读取产生真实有效的新进展，就刷新防空转窗口；不再限制单次长对话总步数。
    pub(super) fn replenish(&mut self, remaining: u64, upstream_bytes: u64) -> u64 {
        if !self.adaptive || upstream_bytes == 0 { return 0; }
        let grant = self.window.saturating_sub(remaining);
        self.granted = self.granted.saturating_add(grant);
        grant
    }

    pub(super) fn snapshot(&self, remaining: Option<u64>) -> Value {
        json!({ "adaptive": self.adaptive, "fuelGranted": self.granted,
            "fuelConsumed": remaining.map(|remaining| self.granted.saturating_sub(remaining)),
            "fuelRemaining": remaining, "fuelHardLimit": (!self.adaptive).then_some(self.window),
            "fuelWindowLimit": self.window })
    }

    pub(super) fn exhausted(&self) -> &'static str {
        if self.adaptive { "fuel_window" } else { "fuel_total" }
    }
}

pub(super) fn contract() -> Value {
    json!({ "owner": "host", "adaptiveClass": "conversation", "baseFuel": BASE_FUEL,
        "inputFuelPerByte": INPUT_FUEL_PER_BYTE, "maxInputFuel": MAX_INPUT_FUEL,
        "windowReplenish": "reset_on_progress", "maxTotalFuel": Value::Null,
        "maxAvailableFuel": MAX_INPUT_FUEL, "yieldInterval": YIELD_INTERVAL,
        "progress": "new_upstream_payload_once", "outputGrantsFuel": false,
        "heartbeatsGrantFuel": false, "verificationAdaptive": false })
}
