#[cfg(feature = "external")]
use zbus::zvariant::{DeserializeDict, SerializeDict, Type, as_value};

use crate::{ByteData, FungibleMemory, MemoryAllocationError, api};

#[derive(Debug)]
#[cfg_attr(
    feature = "external",
    derive(Type, serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "external", zvariant(signature = "dict"))]
#[cfg_attr(
    feature = "external",
    serde(bound(
        serialize = "B: ByteData + serde::Serialize + zbus::zvariant::Type + 'static",
        deserialize = "B: ByteData + serde::de::DeserializeOwned + zbus::zvariant::Type + 'static"
    ))
)]
#[non_exhaustive]
pub struct NewImage<B: ByteData> {
    #[cfg_attr(feature = "external", serde(with = "as_value"))]
    pub image_info: api::ImageDetails<B>,
    #[cfg_attr(feature = "external", serde(with = "as_value"))]
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
        self.image_info.initial_seal().await?;
        for frame in &mut self.frames {
            frame.initial_seal().await?;
        }
        Ok(())
    }

    pub async fn final_seal(&mut self) -> Result<(), MemoryAllocationError> {
        self.image_info.final_seal().await?;
        for frame in &mut self.frames {
            frame.final_seal().await?;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
#[cfg_attr(feature = "external", derive(DeserializeDict, SerializeDict, Type))]
#[cfg_attr(feature = "external", zvariant(signature = "dict"))]
#[non_exhaustive]
pub struct EncodingOptions {
    pub quality: Option<u8>,
    pub compression: Option<u8>,
}

#[derive(Debug)]
#[cfg_attr(
    feature = "external",
    derive(Type, serde::Serialize, serde::Deserialize)
)]
#[cfg_attr(feature = "external", zvariant(signature = "dict"))]
#[cfg_attr(
    feature = "external",
    serde(bound(
        serialize = "B: ByteData + serde::Serialize + zbus::zvariant::Type + 'static",
        deserialize = "B: ByteData + serde::de::DeserializeOwned + zbus::zvariant::Type + 'static"
    ))
)]
#[non_exhaustive]
pub struct EncodedImage<B: ByteData> {
    #[cfg_attr(feature = "external", serde(with = "as_value"))]
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
