use gio::glib;
use gio::prelude::{IsA, *};
pub use glycin_utils::operations::{Operation, Operations};
use glycin_utils::{BinaryData, BitChanges, SafeConversion, SparseEditorOutput};

use crate::api_common::*;
use crate::error::ResultExt;
use crate::{util, Error, ErrorCtx};

/// Image edit builder
#[derive(Debug)]
pub struct Editor {
    file: gio::File,
    cancellable: gio::Cancellable,
    pub(crate) sandbox_selector: SandboxSelector,
}

static_assertions::assert_impl_all!(Editor: Send, Sync);

impl Editor {
    /// Create an editor.
    pub fn new(file: gio::File) -> Self {
        Self {
            file,
            cancellable: gio::Cancellable::new(),
            sandbox_selector: SandboxSelector::default(),
        }
    }

    /// Sets the method by which the sandbox mechanism is selected.
    ///
    /// The default without calling this function is [`SandboxSelector::Auto`].
    pub fn sandbox_selector(&mut self, sandbox_selector: SandboxSelector) -> &mut Self {
        self.sandbox_selector = sandbox_selector;
        self
    }

    /// Set [`Cancellable`](gio::Cancellable) to cancel any editing operations.
    pub fn cancellable(&mut self, cancellable: impl IsA<gio::Cancellable>) -> &mut Self {
        self.cancellable = cancellable.upcast();
        self
    }

    /// Apply operations to the image with a potentially sparse result.
    ///
    /// Some operations like rotation can be in some cases be conducted by only
    /// changing one or a few bytes in a file. We call these cases *sparse* and
    /// a [`SparseEdit::Sparse`] is returned.
    pub async fn apply_sparse(self, operations: Operations) -> Result<SparseEdit, ErrorCtx> {
        let process_context = spin_up(&self.file, &self.cancellable, &self.sandbox_selector)
            .await
            .err_no_context()?;

        let process = process_context.process;

        let editor_output = process
            .editor_apply_sparse(
                &process_context.gfile_worker,
                process_context.base_dir,
                &operations,
            )
            .await
            .err_context(&process)?;

        SparseEdit::try_from(editor_output).err_no_context()
    }

    /// Apply operations to the image
    pub async fn apply_complete(self, operations: &Operations) -> Result<BinaryData, ErrorCtx> {
        let editor_output = self.apply_complete_full(operations).await?;
        Ok(editor_output.data)
    }

    /// Apply operations to the image
    pub async fn apply_complete_full(self, operations: &Operations) -> Result<Edit, ErrorCtx> {
        let process_context = spin_up(&self.file, &self.cancellable, &self.sandbox_selector)
            .await
            .err_no_context()?;

        let process = process_context.process;

        let editor_output = process
            .editor_apply_complete(
                &process_context.gfile_worker,
                process_context.base_dir,
                operations,
            )
            .await
            .err_context(&process)?;

        Ok(Edit {
            data: editor_output.data,
            info: editor_output.info,
        })
    }
}

#[derive(Debug)]
/// An image change that is potentially sparse.
///
/// See also: [`Editor::apply_sparse()`]
pub enum SparseEdit {
    /// The operations can be applied to the image via only changing a few
    /// bytes. The [`apply_to()`](Self::apply_to()) function can be used to
    /// apply these changes.
    Sparse(BitChanges),
    /// The operations require to completely rewrite the image.
    Complete(BinaryData),
}

#[derive(Debug)]
pub struct Edit {
    data: BinaryData,
    info: glycin_utils::EditorOutputInfo,
}

impl Edit {
    pub fn data(&self) -> BinaryData {
        self.data.clone()
    }

    pub fn is_lossless(&self) -> bool {
        self.info.lossless
    }
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
/// Whether an image could be changed via the chosen method.
pub enum EditOutcome {
    Changed,
    Unchanged,
}

impl SparseEdit {
    /// Apply sparse changes if applicable.
    ///
    /// If the type does not carry sparse changes, the function will return an
    /// [`EditOutcome::Unchanged`] and the complete image needs to be rewritten.
    pub async fn apply_to(&self, file: gio::File) -> Result<EditOutcome, Error> {
        match self {
            Self::Sparse(bit_changes) => {
                let bit_changes = bit_changes.clone();
                util::spawn_blocking(move || {
                    let stream = file.open_readwrite(gio::Cancellable::NONE)?;
                    let output_stream = stream.output_stream();
                    for change in bit_changes.changes {
                        stream.seek(
                            change.offset.try_i64()?,
                            glib::SeekType::Set,
                            gio::Cancellable::NONE,
                        )?;
                        let (_, err) =
                            output_stream.write_all(&[change.new_value], gio::Cancellable::NONE)?;

                        if let Some(err) = err {
                            return Err(err.into());
                        }
                    }
                    Ok(EditOutcome::Changed)
                })
                .await
            }
            Self::Complete(_) => Ok(EditOutcome::Unchanged),
        }
    }
}

impl TryFrom<SparseEditorOutput> for SparseEdit {
    type Error = Error;

    fn try_from(value: SparseEditorOutput) -> std::result::Result<Self, Self::Error> {
        if value.bit_changes.is_some() && value.data.is_some() {
            Err(Error::RemoteError(
                glycin_utils::RemoteError::InternalLoaderError(
                    "Sparse editor output with 'bit_changes' and 'data' returned.".into(),
                ),
            ))
        } else if let Some(bit_changes) = value.bit_changes {
            Ok(Self::Sparse(bit_changes))
        } else if let Some(data) = value.data {
            Ok(Self::Complete(data))
        } else {
            Err(Error::RemoteError(
                glycin_utils::RemoteError::InternalLoaderError(
                    "Sparse editor output with neither 'bit_changes' nor 'data' returned.".into(),
                ),
            ))
        }
    }
}
