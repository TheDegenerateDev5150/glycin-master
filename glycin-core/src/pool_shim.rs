use std::sync::Arc;

#[derive(Debug)]
pub struct Pool {}

impl Pool {
    pub fn global() -> Arc<Self> {
        Arc::new(Self {})
    }
}

pub struct PoolConfig {}
