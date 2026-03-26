use std::ops::{Deref, DerefMut};

use crate::{ByteData, FungibleMemory, MemoryAllocationError};

#[derive(Debug, Clone)]
pub struct LocalMemory(Vec<u8>);

impl LocalMemory {
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl ByteData for LocalMemory {
    fn new(size: u64) -> std::io::Result<Self> {
        Ok(Self(vec![0; size as usize]))
    }

    fn into_fungible(self) -> FungibleMemory {
        FungibleMemory::LocalMemory(self.0)
    }

    #[cfg(feature = "external")]
    fn from_shared(shared: crate::SharedMemory) -> Self {
        Self(shared.to_vec())
    }

    fn into_other<O: ByteData>(self) -> Result<O, MemoryAllocationError> {
        O::try_from_vec(self.0)
    }

    fn try_from_vec(value: Vec<u8>) -> Result<Self, MemoryAllocationError> {
        Ok(Self(value))
    }

    fn try_from_slice(value: &[u8]) -> Result<Self, MemoryAllocationError> {
        Ok(Self(value.to_vec()))
    }

    async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        Ok(())
    }

    async fn initial_seal(&mut self) -> Result<(), MemoryAllocationError> {
        Ok(())
    }

    #[cfg(feature = "glib")]
    fn into_gbytes(self) -> Result<glib::Bytes, MemoryAllocationError> {
        Ok(glib::Bytes::from_owned(self.0))
    }
}

impl Deref for LocalMemory {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl DerefMut for LocalMemory {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl From<Vec<u8>> for LocalMemory {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}
