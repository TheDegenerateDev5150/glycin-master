use crate::{SharedMemory, editing};

pub enum ImgBuf {
    MMap(SharedMemory),
    Vec(Vec<u8>),
}

impl ImgBuf {
    pub fn from_shared_memory(shared_memory: SharedMemory) -> Self {
        Self::MMap(shared_memory)
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::MMap(m) => &m,
            Self::Vec(v) => v.as_slice(),
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        match self {
            Self::MMap(m) => m.as_mut(),
            Self::Vec(v) => v.as_mut_slice(),
        }
    }

    pub fn into_vec(self) -> Vec<u8> {
        match self {
            Self::Vec(vec) => vec,
            Self::MMap { .. } => self.to_vec(),
        }
    }

    pub fn resize(self, new_len: i64) -> Result<Self, editing::Error> {
        if self.len() == new_len as usize {
            return Ok(self);
        }

        match self {
            ImgBuf::MMap(shared_memory) => {
                /*
                let borrowed_fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw_fd) };

                // This mmap would have the wrong size after ftruncate
                drop(mmap);

                nix::unistd::ftruncate(
                    borrowed_fd,
                    libc::off_t::try_from(new_len).map_err(|_| DimensionTooLargerError)?,
                )
                .map_err(std::io::Error::from)?;

                // Need a new mmap with correct size
                let mmap = unsafe { memmap::MmapMut::map_mut(raw_fd) }?;

                Ok(ImgBuf::MMap { mmap, raw_fd })
                 */
                todo!()
            }
            Self::Vec(mut vec) => {
                vec.resize(new_len as usize, 0);
                Ok(Self::Vec(vec))
            }
        }
    }
}

impl std::ops::Deref for ImgBuf {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl std::ops::DerefMut for ImgBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}
