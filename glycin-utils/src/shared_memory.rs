use std::ops::{Deref, DerefMut};
use std::os::fd::{AsRawFd, OwnedFd};

use log::warn;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zbus::zvariant;

#[derive(Debug)]
pub struct SharedMemory {
    memfd: OwnedFd,
    mmap: Option<MMapOptions>,
}

#[derive(Debug)]
enum MMapOptions {
    Mutable(memmap::MmapMut),
    ReadOnly(memmap::Mmap),
}

impl Deref for MMapOptions {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Mutable(x) => x.deref(),
            Self::ReadOnly(x) => x.deref(),
        }
    }
}

impl DerefMut for MMapOptions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Mutable(x) => x.deref_mut(),
            Self::ReadOnly(_) => {
                panic!("Shared memory has been sealed and can't be written to anymore.")
            }
        }
    }
}

impl zvariant::Type for SharedMemory {
    const SIGNATURE: &'static zvariant::Signature = &zvariant::Signature::Fd;
}

impl Serialize for SharedMemory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        zvariant::OwnedFd::from(self.memfd.try_clone().unwrap()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SharedMemory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let memfd = zvariant::OwnedFd::deserialize(deserializer)?.into();

        Ok(Self {
            memfd: memfd,
            mmap: None,
        })
    }
}

impl Default for SharedMemory {
    fn default() -> Self {
        todo!()
    }
}

#[derive(Debug)]
pub struct MemoryAllocationError(String);

impl std::fmt::Display for MemoryAllocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MemoryAllocationError {}

pub trait ByteData:
    zvariant::Type + Sized + Deref<Target = [u8]> + DerefMut + Serialize + DeserializeOwned + 'static
{
    fn new(size: u64) -> std::io::Result<Self>;
    fn into_fungible(self) -> FungibleMemory;
    fn into_other<O: ByteData>(self) -> Result<O, MemoryAllocationError>;
    fn from_shared(shared: SharedMemory) -> Self;
    fn try_from_vec(vec: Vec<u8>) -> Result<Self, MemoryAllocationError>;
    fn try_from_slice(slice: &[u8]) -> Result<Self, MemoryAllocationError>;
    fn initial_seal(
        &mut self,
    ) -> impl std::future::Future<Output = Result<(), MemoryAllocationError>> + Send;
    fn final_seal(
        &mut self,
    ) -> impl std::future::Future<Output = Result<(), MemoryAllocationError>> + Send;
    #[cfg(feature = "glib")]
    fn into_gbytes(self) -> Result<glib::Bytes, MemoryAllocationError>;
}

impl ByteData for SharedMemory {
    fn new(size: u64) -> std::io::Result<Self> {
        let (memfd, mmap) = Self::new_memfd(size)?;

        Ok(Self {
            memfd,
            mmap: Some(MMapOptions::Mutable(mmap)),
        })
    }

    fn into_fungible(self) -> FungibleMemory {
        FungibleMemory::SharedMemory(self)
    }

    fn from_shared(shared: SharedMemory) -> Self {
        shared
    }

    fn into_other<O: ByteData>(self) -> Result<O, MemoryAllocationError> {
        Ok(O::from_shared(self))
    }

    fn try_from_vec(value: Vec<u8>) -> Result<Self, MemoryAllocationError> {
        Self::try_from_slice(&value)
    }

    fn try_from_slice(value: &[u8]) -> Result<Self, MemoryAllocationError> {
        let (memfd, mut mmap) = Self::new_memfd(u64::try_from(value.len()).unwrap()).unwrap();

        mmap.copy_from_slice(value.as_ref());

        Ok(Self {
            memfd,
            mmap: Some(MMapOptions::Mutable(mmap)),
        })
    }

    async fn initial_seal(&mut self) -> Result<(), MemoryAllocationError> {
        if self.mmap.is_some() {
            warn!("SharedMemory already got inital seal.");
            return Ok(());
        }

        self.seal(&[memfd::FileSeal::SealShrink, memfd::FileSeal::SealGrow])
            .await
            .map_err(|err| MemoryAllocationError(err.to_string()))?;

        self.add_mut_memmap()?;

        Ok(())
    }

    async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        self.mmap = None;

        self.seal(&[
            memfd::FileSeal::SealShrink,
            memfd::FileSeal::SealGrow,
            memfd::FileSeal::SealWrite,
            memfd::FileSeal::SealSeal,
        ])
        .await
        .map_err(|err| MemoryAllocationError(err.to_string()))?;

        self.add_memmap()?;

        Ok(())
    }

    #[cfg(feature = "glib")]
    fn into_gbytes(self) -> Result<glib::Bytes, MemoryAllocationError> {
        if !matches!(self.mmap, Some(MMapOptions::ReadOnly(_))) {
            panic!("SharedMemory is lacking final seal.");
        }

        use std::os::fd::RawFd;

        pub unsafe fn gbytes_from_mmap(
            raw_fd: RawFd,
        ) -> Result<glib::Bytes, MemoryAllocationError> {
            unsafe {
                let mut error = std::ptr::null_mut();

                let mapped_file =
                    glib::ffi::g_mapped_file_new_from_fd(raw_fd, glib::ffi::GFALSE, &mut error);

                if !error.is_null() {
                    let err: glib::Error = glib::translate::from_glib_full(error);
                    return Err(MemoryAllocationError(err.to_string()));
                };

                let bytes = glib::translate::from_glib_full(glib::ffi::g_mapped_file_get_bytes(
                    mapped_file,
                ));

                glib::ffi::g_mapped_file_unref(mapped_file);

                Ok(bytes)
            }
        }

        unsafe { gbytes_from_mmap(self.memfd.as_raw_fd()) }
    }
}

impl SharedMemory {
    fn new_memfd(size: u64) -> std::io::Result<(OwnedFd, memmap::MmapMut)> {
        let memfd = nix::sys::memfd::memfd_create(
            c"glycin-frame",
            nix::sys::memfd::MFdFlags::MFD_CLOEXEC | nix::sys::memfd::MFdFlags::MFD_ALLOW_SEALING,
        )?;

        nix::unistd::ftruncate(&memfd, size.try_into().expect("Required memory too large"))?;

        let raw_fd = memfd.as_raw_fd();
        let mmap = unsafe { memmap::MmapMut::map_mut(raw_fd) }?;

        Ok((memfd, mmap))
    }

    fn add_mut_memmap(&mut self) -> Result<(), MemoryAllocationError> {
        let mmap: memmap::MmapMut = unsafe { memmap::MmapMut::map_mut(&self.memfd) }
            .map_err(|err| MemoryAllocationError(err.to_string()))?;

        self.mmap = Some(MMapOptions::Mutable(mmap));

        Ok(())
    }

    fn add_memmap(&mut self) -> Result<(), MemoryAllocationError> {
        let mmap: memmap::Mmap = unsafe { memmap::Mmap::map(&self.memfd) }
            .map_err(|err| MemoryAllocationError(err.to_string()))?;

        self.mmap = Some(MMapOptions::ReadOnly(mmap));

        Ok(())
    }

    async fn seal(&self, seals: &[memfd::FileSeal]) -> Result<(), memfd::Error> {
        let raw_fd = self.memfd.as_raw_fd();

        let start = std::time::Instant::now();

        let mfd = memfd::Memfd::try_from_fd(raw_fd).unwrap();
        // Sealing returns a ResourceBusy for SealWrite until no readable
        loop {
            // 🦭
            let seal = mfd.add_seals(seals);

            match seal {
                Ok(_) => break,
                Err(err) if start.elapsed() > std::time::Duration::from_secs(10) => {
                    // Give up after some time and return the error
                    return Err(err);
                }
                Err(_) => {
                    // Try again after short waiting time
                    futures_timer::Delay::new(std::time::Duration::from_millis(1)).await
                }
            }
        }
        std::mem::forget(mfd);

        Ok(())
    }
}

impl Deref for SharedMemory {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.mmap
            .as_ref()
            .expect("SharedMemory haven't been sealed before use.")
    }
}

impl DerefMut for SharedMemory {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.mmap
            .as_mut()
            .expect("SharedMemory haven't been sealed before use.")
    }
}

#[derive(Debug, zvariant::Type, Serialize, Deserialize, Clone)]
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

    fn from_shared(shared: SharedMemory) -> Self {
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

#[derive(Debug, Serialize, Deserialize)]
pub enum FungibleMemory {
    SharedMemory(SharedMemory),
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

    fn from_shared(shared: SharedMemory) -> Self {
        FungibleMemory::SharedMemory(shared)
    }

    fn into_other<O: ByteData>(self) -> Result<O, MemoryAllocationError> {
        match self {
            Self::LocalMemory(local) => O::try_from_vec(local),
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
        if let Self::SharedMemory(shared) = self {
            shared.initial_seal().await?;
        }

        Ok(())
    }

    async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        if let Self::SharedMemory(shared) = self {
            shared.final_seal().await?;
        }

        Ok(())
    }

    #[cfg(feature = "glib")]
    fn into_gbytes(self) -> Result<glib::Bytes, MemoryAllocationError> {
        match self {
            FungibleMemory::LocalMemory(local) => Ok(glib::Bytes::from_owned(local)),
            FungibleMemory::SharedMemory(shared) => shared.into_gbytes(),
        }
    }
}

impl Deref for FungibleMemory {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::LocalMemory(local) => local,
            Self::SharedMemory(shared) => shared,
        }
    }
}

impl DerefMut for FungibleMemory {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::LocalMemory(local) => local,
            Self::SharedMemory(shared) => shared,
        }
    }
}

impl From<Vec<u8>> for FungibleMemory {
    fn from(value: Vec<u8>) -> Self {
        Self::LocalMemory(value)
    }
}
