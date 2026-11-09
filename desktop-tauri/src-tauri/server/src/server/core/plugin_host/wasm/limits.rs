use wasmtime::{ResourceLimiter, StoreLimits};

pub(super) struct Limits {
    pub store: StoreLimits,
    pub exhausted: bool,
}

impl ResourceLimiter for Limits {
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        let result = self.store.memory_growing(current, desired, maximum);
        if !matches!(result, Ok(true)) { self.exhausted = true; }
        result
    }
    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        let result = self.store.table_growing(current, desired, maximum);
        if !matches!(result, Ok(true)) { self.exhausted = true; }
        result
    }
    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.exhausted = true;
        self.store.memory_grow_failed(error)
    }
    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.exhausted = true;
        self.store.table_grow_failed(error)
    }
    fn instances(&self) -> usize { self.store.instances() }
    fn tables(&self) -> usize { self.store.tables() }
    fn memories(&self) -> usize { self.store.memories() }
}
