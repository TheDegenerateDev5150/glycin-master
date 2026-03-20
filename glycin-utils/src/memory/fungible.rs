use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};
use zbus::zvariant;

use crate::{ByteData, MemoryAllocationError};

#[derive(Debug, Serialize, Deserialize)]
pub enum FungibleMemory {
    #[cfg(feature = "external")]
    SharedMemory(crate::SharedMemory),
    LocalMemory(Vec<u8>),
}

impl zvariant::Type for FungibleMemory {
    const SIGNATURE: &'static zvariant::Signature = zvariant::OwnedFd::SIGNATURE;
}

impl FungibleMemory {
    pub fn from_vec(vec: Vec<u8>) -> Self {
        FungibleMemory::LocalMemory(vec)
    }
}

impl ByteData for FungibleMemory {
    fn new(size: u64) -> std::io::Result<Self> {
        Ok(Self::LocalMemory(vec![0; size as usize]))
    }

    fn into_fungible(self) -> FungibleMemory {
        self
    }

    #[cfg(feature = "external")]
    fn from_shared(shared: crate::SharedMemory) -> Self {
        FungibleMemory::SharedMemory(shared)
    }

    fn into_other<O: ByteData>(self) -> Result<O, MemoryAllocationError> {
        match self {
            Self::LocalMemory(local) => O::try_from_vec(local),
            #[cfg(feature = "external")]
            Self::SharedMemory(shared) => Ok(O::from_shared(shared)),
        }
    }

    fn try_from_vec(value: Vec<u8>) -> Result<Self, MemoryAllocationError> {
        Ok(Self::LocalMemory(value))
    }

    fn try_from_slice(value: &[u8]) -> Result<Self, MemoryAllocationError> {
        Ok(Self::LocalMemory(value.to_vec()))
    }

    async fn initial_seal(&mut self) -> Result<(), MemoryAllocationError> {
        #[cfg(feature = "external")]
        if let Self::SharedMemory(shared) = self {
            shared.initial_seal().await?;
        }

        Ok(())
    }

    async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        #[cfg(feature = "external")]
        if let Self::SharedMemory(shared) = self {
            shared.final_seal().await?;
        }

        Ok(())
    }

    #[cfg(feature = "glib")]
    fn into_gbytes(self) -> Result<glib::Bytes, MemoryAllocationError> {
        match self {
            FungibleMemory::LocalMemory(local) => Ok(glib::Bytes::from_owned(local)),
            #[cfg(feature = "external")]
            FungibleMemory::SharedMemory(shared) => shared.into_gbytes(),
        }
    }
}

impl Deref for FungibleMemory {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::LocalMemory(local) => local,
            #[cfg(feature = "external")]
            Self::SharedMemory(shared) => shared,
        }
    }
}

impl DerefMut for FungibleMemory {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::LocalMemory(local) => local,
            #[cfg(feature = "external")]
            Self::SharedMemory(shared) => shared,
        }
    }
}

impl From<Vec<u8>> for FungibleMemory {
    fn from(value: Vec<u8>) -> Self {
        Self::LocalMemory(value)
    }
}
