// Copyright (c) 2024 GNOME Foundation Inc.

use std::any::Any;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use glycin_common::Operations;
use serde::{Deserialize, Serialize};
use zbus::zvariant::{DeserializeDict, OwnedObjectPath, SerializeDict, Type, as_value};

use crate::dbus_types::{self, *};
use crate::error::*;
use crate::{ByteData, FungibleMemory, SharedMemory};

#[derive(DeserializeDict, SerializeDict, Type, Debug)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct EditRequest {
    pub operations: SharedMemory,
}

impl EditRequest {
    pub fn for_operations(operations: &Operations) -> Result<Self, RemoteError> {
        let operations = operations
            .to_message_pack()
            .expected_error()
            .map_err(|x| x.into_editor_error())?;
        let operations = SharedMemory::try_from_vec(operations).unwrap();
        Ok(Self { operations })
    }

    pub fn operations(&self) -> Result<Operations, RemoteError> {
        let operations = Operations::from_slice(&self.operations)
            .expected_error()
            .map_err(|x| x.into_editor_error())?;

        Ok(operations)
    }
}

/// Result of a sparse editor operation
///
/// This either contains `byte_changes` or `data`, depending on whether a sparse
/// application of the operations was possible.
#[derive(Deserialize, Serialize, Type, Debug)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct SparseEditorOutput<B: ByteData> {
    #[serde(
        with = "as_value::optional",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub byte_changes: Option<ByteChanges>,
    #[serde(
        with = "as_value::optional",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub data: Option<B>,
    #[serde(with = "as_value")]
    pub info: EditorOutputInfo,
}

impl<B: ByteData> SparseEditorOutput<B> {
    pub fn byte_changes(byte_changes: ByteChanges) -> Self {
        SparseEditorOutput {
            byte_changes: Some(byte_changes),
            data: None,
            info: EditorOutputInfo { lossless: true },
        }
    }
}

impl<B: ByteData> From<CompleteEditorOutput<B>> for SparseEditorOutput<B> {
    fn from(value: CompleteEditorOutput<B>) -> Self {
        Self {
            byte_changes: None,
            data: Some(value.data),
            info: value.info,
        }
    }
}

#[derive(DeserializeDict, SerializeDict, Type, Debug, Clone)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct ByteChanges {
    pub changes: Vec<ByteChange>,
}

#[derive(Deserialize, Serialize, Type, Debug, Clone)]
pub struct ByteChange {
    pub offset: u64,
    pub new_value: u8,
}

impl ByteChanges {
    pub fn from_slice(changes: &[(u64, u8)]) -> Self {
        ByteChanges {
            changes: changes
                .iter()
                .map(|(offset, new_value)| ByteChange {
                    offset: *offset,
                    new_value: *new_value,
                })
                .collect(),
        }
    }

    pub fn apply(&self, data: &mut [u8]) {
        let mut cur = Cursor::new(data);
        for change in self.changes.iter() {
            cur.seek(SeekFrom::Start(change.offset)).unwrap();
            cur.write_all(&[change.new_value]).unwrap();
        }
    }
}

#[derive(Deserialize, Serialize, Type, Debug)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct CompleteEditorOutput<B: ByteData> {
    #[serde(with = "as_value")]
    pub data: B,
    #[serde(with = "as_value")]
    pub info: EditorOutputInfo,
}

impl<B: ByteData> CompleteEditorOutput<B> {
    pub fn new(data: B) -> Self {
        Self {
            data,
            info: Default::default(),
        }
    }

    pub fn new_lossless(data: Vec<u8>) -> Result<Self, ProcessError> {
        let data = B::try_from_vec(data).expected_error()?;
        let info = EditorOutputInfo { lossless: true };
        Ok(Self { data, info })
    }

    pub fn into_fungible(self) -> CompleteEditorOutput<FungibleMemory> {
        CompleteEditorOutput {
            data: self.data.into_fungible(),
            info: self.info,
        }
    }
}

#[derive(DeserializeDict, SerializeDict, Type, Debug, Default, Clone)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct EditorOutputInfo {
    /// Operation is considered to be lossless
    ///
    /// Operations are considered lossless when all metadata are kept, no image
    /// data is lost, and no image quality is lost.
    pub lossless: bool,
}

pub struct Editor<E: EditorImplementation> {
    pub editor: PhantomData<E>,
    pub image_id: Mutex<u64>,
}

/// D-Bus interface for image editors
#[zbus::interface(name = "org.gnome.glycin.Editor")]
impl<E: EditorImplementation> Editor<E> {
    async fn create(
        &self,
        mime_type: String,
        new_image: NewImage<SharedMemory>,
        encoding_options: EncodingOptions,
    ) -> Result<EncodedImage<SharedMemory>, RemoteError> {
        E::create(mime_type, new_image, encoding_options).map_err(|x| x.into_editor_error())
    }

    async fn edit(
        &self,
        init_request: InitRequest,
        #[zbus(connection)] dbus_connection: &zbus::Connection,
    ) -> Result<dbus_types::RemoteEditableImage, RemoteError> {
        let fd = OwnedFd::from(init_request.fd);
        let stream = UnixStream::from(fd);

        let editor_state = E::edit(stream, init_request.mime_type, init_request.details)
            .map_err(|x| x.into_loader_error())?;

        let image_id = {
            let lock = self.image_id.lock();
            let mut image_id = match lock {
                Ok(id) => id,
                Err(err) => return Err(RemoteError::InternalLoaderError(err.to_string())),
            };
            let id = *image_id;
            *image_id = id + 1;
            id
        };

        let path =
            OwnedObjectPath::try_from(format!("/org/gnome/glycin/editable_image/{image_id}"))
                .internal_error()
                .map_err(|x| x.into_loader_error())?;

        let dbus_image = dbus_types::RemoteEditableImage::new(path.clone());

        dbus_connection
            .object_server()
            .at(
                &path,
                EditableImage {
                    editor_implementation: Arc::new(Box::new(editor_state)),
                    path: path.clone(),
                    dropped: Default::default(),
                },
            )
            .await
            .internal_error()
            .map_err(|x| x.into_loader_error())?;

        Ok(dbus_image)
    }
}

pub struct EditableImage<E: EditorImplementation> {
    pub editor_implementation: Arc<Box<E>>,
    pub path: OwnedObjectPath,
    dropped: async_lock::OnceCell<()>,
}

#[zbus::interface(name = "org.gnome.glycin.EditableImage")]
impl<E: EditorImplementation> EditableImage<E> {
    async fn apply_sparse(
        &self,
        edit_request: EditRequest,
    ) -> Result<SparseEditorOutput<SharedMemory>, RemoteError> {
        let operations = edit_request.operations()?;

        let editor_implementation = self.editor_implementation.clone();
        let mut editor_output = blocking::unblock(move || {
            editor_implementation
                .apply_sparse(operations)
                .map_err(|x| x.into_loader_error())
        })
        .fuse();

        futures_util::select! {
            result = editor_output => result,
            _ = self.dropped.wait().fuse() => Err(RemoteError::Aborted),
        }
    }

    /// Same as [`Self::apply()`] but without potential to return sparse changes
    async fn apply_complete(
        &self,
        edit_request: EditRequest,
    ) -> Result<CompleteEditorOutput<SharedMemory>, RemoteError> {
        let operations = edit_request.operations()?;

        let editor_implementation = self.editor_implementation.clone();
        let mut editor_output = blocking::unblock(move || {
            editor_implementation
                .apply_complete(operations)
                .map_err(|x| x.into_loader_error())
        })
        .fuse();

        futures_util::select! {
            result = editor_output => result,
            _ = self.dropped.wait().fuse() => Err(RemoteError::Aborted),
        }
    }

    async fn done(
        &self,
        #[zbus(object_server)] object_server: &zbus::ObjectServer,
    ) -> Result<(), RemoteError> {
        log::debug!("Disconnecting {}", self.path);
        let removed = object_server
            .remove::<EditableImage<E>, _>(&self.path)
            .await?;
        if removed {
            log::debug!("Removed {}", self.path);
        } else {
            log::error!("Failed to remove {}", self.path);
        }
        let _ = self.dropped.set(()).await;
        Ok(())
    }
}

/// Implement this trait to create an image editor
pub trait EditorImplementation: Send + Sync + Sized + 'static {
    const USEABLE: bool = true;

    fn edit<S: Read + Any>(
        stream: S,
        mime_type: String,
        details: InitializationDetails,
    ) -> Result<Self, ProcessError>;

    fn create<B: ByteData>(
        mime_type: String,
        new_image: NewImage<B>,
        encoding_options: EncodingOptions,
    ) -> Result<EncodedImage<B>, ProcessError>;

    fn apply_sparse<B: ByteData>(
        &self,
        operations: Operations,
    ) -> Result<SparseEditorOutput<B>, ProcessError> {
        let complete = Self::apply_complete(self, operations)?;

        Ok(SparseEditorOutput::from(complete))
    }

    fn apply_complete<B: ByteData>(
        &self,
        operations: Operations,
    ) -> Result<CompleteEditorOutput<B>, ProcessError>;
}

/// Give a `None` for a non-existent `EditorImplementation`
pub enum VoidEditorImplementation {}

impl EditorImplementation for VoidEditorImplementation {
    const USEABLE: bool = false;

    fn edit<S: Read>(
        _stream: S,
        _mime_type: String,
        _details: InitializationDetails,
    ) -> Result<Self, ProcessError> {
        unreachable!()
    }

    fn create<B: ByteData>(
        _mime_type: String,
        _new_image: NewImage<B>,
        _encoding_options: EncodingOptions,
    ) -> Result<EncodedImage<B>, ProcessError> {
        unreachable!()
    }

    fn apply_complete<B: ByteData>(
        &self,
        _operations: Operations,
    ) -> Result<CompleteEditorOutput<B>, ProcessError> {
        unreachable!()
    }
}
