// Copyright (c) 2024 GNOME Foundation Inc.

use std::io::{Cursor, Seek, SeekFrom, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use zbus::zvariant::{DeserializeDict, SerializeDict, Type};

use crate::dbus::*;
use crate::error::*;
use crate::operations::Operations;

#[derive(DeserializeDict, SerializeDict, Type, Debug)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct EditRequest {
    pub operations: BinaryData,
}

impl EditRequest {
    pub fn for_operations(operations: &Operations) -> Result<Self, RemoteError> {
        let operations = operations
            .to_message_pack()
            .expected_error()
            .map_err(|x| x.into_editor_error())?;
        let operations = BinaryData::from_data(operations).map_err(|x| x.into_editor_error())?;
        Ok(Self { operations })
    }

    pub fn operations(&self) -> Result<Operations, RemoteError> {
        let binary_data = self
            .operations
            .get()
            .expected_error()
            .map_err(|x| x.into_editor_error())?;

        let operations = Operations::from_slice(&binary_data)
            .expected_error()
            .map_err(|x| x.into_editor_error())?;

        Ok(operations)
    }
}

/// Result of a sparse editor operation
///
/// This either contains `byte_changes` or `data`, depending on whether a sparse
/// application of the operations was possible.
#[derive(DeserializeDict, SerializeDict, Type, Debug, Clone)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct SparseEditorOutput {
    pub byte_changes: Option<ByteChanges>,
    pub data: Option<BinaryData>,
    pub info: EditorOutputInfo,
}

impl SparseEditorOutput {
    pub fn byte_changes(byte_changes: ByteChanges) -> Self {
        SparseEditorOutput {
            byte_changes: Some(byte_changes),
            data: None,
            info: EditorOutputInfo { lossless: true },
        }
    }
}

impl From<CompleteEditorOutput> for SparseEditorOutput {
    fn from(value: CompleteEditorOutput) -> Self {
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
            cur.write(&[change.new_value]).unwrap();
        }
    }
}

#[derive(DeserializeDict, SerializeDict, Type, Debug, Clone)]
#[zvariant(signature = "dict")]
#[non_exhaustive]
pub struct CompleteEditorOutput {
    pub data: BinaryData,
    pub info: EditorOutputInfo,
}

impl CompleteEditorOutput {
    pub fn new(data: BinaryData) -> Self {
        Self {
            data,
            info: Default::default(),
        }
    }

    pub fn new_lossless(data: Vec<u8>) -> Result<Self, ProcessError> {
        let data = BinaryData::from_data(data)?;
        let info = EditorOutputInfo { lossless: true };
        Ok(Self { data, info })
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

pub struct Editor {
    pub editor: Mutex<Box<dyn EditorImplementation>>,
}

/// D-Bus interface for image editors
#[zbus::interface(name = "org.gnome.glycin.Editor")]
impl Editor {
    async fn apply(
        &self,
        init_request: InitRequest,
        edit_request: EditRequest,
    ) -> Result<SparseEditorOutput, RemoteError> {
        let fd: OwnedFd = OwnedFd::from(init_request.fd);
        let stream = UnixStream::from(fd);
        let operations = edit_request.operations()?;

        let image_info = self
            .get_editor()?
            .apply_sparse(
                stream,
                init_request.mime_type,
                init_request.details,
                operations,
            )
            .map_err(|x| x.into_editor_error())?;

        Ok(image_info)
    }

    /// Same as [`Self::apply()`] but without potential to return sparse changes
    async fn apply_complete(
        &self,
        init_request: InitRequest,
        edit_request: EditRequest,
    ) -> Result<CompleteEditorOutput, RemoteError> {
        let fd: OwnedFd = OwnedFd::from(init_request.fd);
        let stream = UnixStream::from(fd);
        let operations = edit_request.operations()?;

        let image_info = self
            .get_editor()?
            .apply_complete(
                stream,
                init_request.mime_type,
                init_request.details,
                operations,
            )
            .map_err(|x| x.into_editor_error())?;

        Ok(image_info)
    }
}

impl Editor {
    pub fn get_editor(&self) -> Result<MutexGuard<Box<dyn EditorImplementation>>, RemoteError> {
        self.editor.lock().map_err(|err| {
            RemoteError::InternalLoaderError(format!("Failed to lock editor for operation: {err}"))
        })
    }
}

/// Implement this trait to create an image editor
pub trait EditorImplementation: Send {
    fn apply_sparse(
        &self,
        stream: UnixStream,
        mime_type: String,
        details: InitializationDetails,
        operations: Operations,
    ) -> Result<SparseEditorOutput, ProcessError> {
        let complete = self.apply_complete(stream, mime_type, details, operations)?;

        Ok(SparseEditorOutput::from(complete))
    }

    fn apply_complete(
        &self,
        stream: UnixStream,
        mime_type: String,
        details: InitializationDetails,
        operations: Operations,
    ) -> Result<CompleteEditorOutput, ProcessError>;
}

/// Give a `None` for a non-existent `EditorImplementation`
pub fn void_editor_none() -> Option<impl EditorImplementation> {
    enum Void {}

    impl EditorImplementation for Void {
        fn apply_sparse(
            &self,
            _stream: UnixStream,
            _mime_type: String,
            _details: InitializationDetails,
            _operations: Operations,
        ) -> Result<SparseEditorOutput, ProcessError> {
            match *self {}
        }

        fn apply_complete(
            &self,
            _stream: UnixStream,
            _mime_type: String,
            _details: InitializationDetails,
            _operations: Operations,
        ) -> Result<CompleteEditorOutput, ProcessError> {
            match *self {}
        }
    }

    None::<Void>
}
