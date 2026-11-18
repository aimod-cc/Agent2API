use wasmtime::{ResourceLimiter, StoreLimits};

pub(super) struct Limits {
    pub store: StoreLimits,
    pub exhausted: bool,
    pub reason: Option<&'static str>,
    pub memory: crate::server::core::runtime_capacity::MemoryBudget,
    pub baseline: usize,
    pub linear_bytes: usize,
    pub table_bytes: usize,
}

impl ResourceLimiter for Limits {
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        let result = self.store.memory_growing(current, desired, maximum);
        if matches!(result, Ok(true)) && self.memory.resize(self.baseline.saturating_add(desired).saturating_add(self.table_bytes)).is_err() {
            self.exhausted = true;
            self.reason = Some("host_memory");
            return Ok(false);
        }
        if matches!(result, Ok(true)) { self.linear_bytes = desired; }
        if !matches!(result, Ok(true)) { self.exhausted = true; self.reason = Some("memory"); }
        result
    }
    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        let result = self.store.table_growing(current, desired, maximum);
        if matches!(result, Ok(true)) {
            let table_bytes = self.table_bytes.saturating_add(desired.saturating_sub(current).saturating_mul(16));
            if self.memory.resize(self.baseline.saturating_add(self.linear_bytes).saturating_add(table_bytes)).is_err() {
                self.exhausted = true;
                self.reason = Some("host_memory");
                return Ok(false);
            }
            self.table_bytes = table_bytes;
        }
        if !matches!(result, Ok(true)) { self.exhausted = true; self.reason = Some("table"); }
        result
    }
    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.exhausted = true;
        self.reason.get_or_insert("memory");
        self.store.memory_grow_failed(error)
    }
    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.exhausted = true;
        self.reason.get_or_insert("table");
        self.store.table_grow_failed(error)
    }
    fn instances(&self) -> usize { self.store.instances() }
    fn tables(&self) -> usize { self.store.tables() }
    fn memories(&self) -> usize { self.store.memories() }
}
