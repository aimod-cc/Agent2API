use serde_json::{json, Value};

pub(super) const BASE_FUEL: u64 = 2_000_000_000;
pub(super) const MAX_INPUT_FUEL: u64 = 8_000_000_000;
pub(super) const MAX_TOTAL_FUEL: u64 = 20_000_000_000;
pub(super) const INPUT_FUEL_PER_BYTE: u64 = 2048;
pub(super) const UPSTREAM_FUEL_PER_BYTE: u64 = 4096;
pub(super) const YIELD_INTERVAL: u64 = 100_000;

pub(super) struct Budget {
    adaptive: bool,
    base: u64,
    granted: u64,
    window: u64,
    maximum: u64,
}

impl Budget {
    pub(super) fn new(adaptive: bool, verification: bool) -> Self {
        let adaptive = adaptive && !verification;
        let base = if verification { super::verification::FUEL } else { BASE_FUEL };
        Self { adaptive, base, granted: 0, window: base,
            maximum: if adaptive { MAX_TOTAL_FUEL } else { base } }
    }

    pub(super) fn input(&mut self, bytes: usize) -> u64 {
        self.granted = if self.adaptive {
            self.base.saturating_add((bytes as u64).saturating_mul(INPUT_FUEL_PER_BYTE)).min(MAX_INPUT_FUEL)
        } else { self.base };
        self.window = self.granted;
        self.granted
    }

    // 只增加原上游读取产生的新额度；不恢复已经消耗的燃料，不在普通宿主调用时重置。
    pub(super) fn replenish(&mut self, remaining: u64, upstream_bytes: u64) -> u64 {
        if !self.adaptive || upstream_bytes == 0 { return 0; }
        let grant = upstream_bytes.saturating_mul(UPSTREAM_FUEL_PER_BYTE)
            .min(self.maximum.saturating_sub(self.granted))
            .min(self.window.saturating_sub(remaining));
        self.granted += grant;
        grant
    }

    pub(super) fn snapshot(&self, remaining: Option<u64>) -> Value {
        json!({ "adaptive": self.adaptive, "fuelGranted": self.granted,
            "fuelConsumed": remaining.map(|remaining| self.granted.saturating_sub(remaining)),
            "fuelRemaining": remaining, "fuelHardLimit": self.maximum, "fuelWindowLimit": self.window })
    }

    pub(super) fn exhausted(&self) -> &'static str {
        if self.adaptive && self.granted < self.maximum { "fuel_window" } else { "fuel_total" }
    }
}

pub(super) fn contract() -> Value {
    json!({ "owner": "host", "adaptiveClass": "conversation", "baseFuel": BASE_FUEL,
        "inputFuelPerByte": INPUT_FUEL_PER_BYTE, "maxInputFuel": MAX_INPUT_FUEL,
        "upstreamFuelPerByte": UPSTREAM_FUEL_PER_BYTE, "maxTotalFuel": MAX_TOTAL_FUEL,
        "maxAvailableFuel": MAX_INPUT_FUEL, "yieldInterval": YIELD_INTERVAL,
        "progress": "new_upstream_payload_once", "outputGrantsFuel": false,
        "heartbeatsGrantFuel": false, "verificationAdaptive": false })
}
