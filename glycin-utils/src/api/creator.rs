use serde::{Deserialize, Serialize};
use zbus::zvariant::{DeserializeDict, SerializeDict, Type, as_value};

use crate::{ByteData, FungibleMemory, MemoryAllocationError, api};

#[derive(Deserialize, Serialize, Type, Debug)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct NewImage<B: ByteData> {
    #[serde(with = "as_value")]
    pub image_info: api::ImageDetails<B>,
    #[serde(with = "as_value")]
    pub frames: Vec<api::Frame<B>>,
}

impl<B: ByteData> NewImage<B> {
    pub fn new(image_info: api::ImageDetails<B>, frames: Vec<api::Frame<B>>) -> Self {
        Self { image_info, frames }
    }

    pub fn into_other<O: ByteData>(self) -> Result<NewImage<O>, MemoryAllocationError> {
        Ok(NewImage {
            image_info: self.image_info.into_other()?,
            frames: self
                .frames
                .into_iter()
                .map(|x| x.into_other::<O>())
                .collect::<Result<_, _>>()?,
        })
    }

    pub async fn initial_seal(&mut self) -> Result<(), MemoryAllocationError> {
        self.image_info.initial_seal().await
    }

    pub async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        self.image_info.final_seal().await
    }
}

#[derive(DeserializeDict, SerializeDict, Type, Debug, Default)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct EncodingOptions {
    pub quality: Option<u8>,
    pub compression: Option<u8>,
}

#[derive(Deserialize, Serialize, Type, Debug)]
#[zvariant(signature = "dict")]
#[serde(bound(deserialize = "B: ByteData"))]
#[non_exhaustive]
pub struct EncodedImage<B: ByteData> {
    #[serde(with = "as_value")]
    pub data: B,
}

impl<B: ByteData> EncodedImage<B> {
    pub fn new(data: B) -> Self {
        Self { data }
    }

    pub async fn inital_seal(&mut self) -> Result<(), MemoryAllocationError> {
        self.data.initial_seal().await
    }

    pub async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        self.data.final_seal().await
    }

    pub fn into_fungible(self) -> EncodedImage<FungibleMemory> {
        EncodedImage {
            data: self.data.into_fungible(),
        }
    }
}
