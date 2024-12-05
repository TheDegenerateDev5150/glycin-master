// Copyright (c) 2024 GNOME Foundation Inc.

//! Internal DBus API

use std::io::{BufRead, Read};
use std::marker::PhantomData;
use std::mem;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use futures_util::{future, FutureExt};
use gio::glib;
use gio::prelude::*;
use glycin_utils::memory_format::MemoryFormatInfo;
use glycin_utils::operations::Operations;
use glycin_utils::{
    CompleteEditorOutput, DimensionTooLargerError, EditRequest, Frame, FrameRequest, ImageInfo,
    ImgBuf, InitRequest, InitializationDetails, MemoryFormat, RemoteError, SafeConversion,
    SafeMath, SparseEditorOutput,
};
use gufo_common::cicp::Cicp;
use memmap::MmapMut;
use nix::sys::signal;
use zbus::zvariant;

use crate::api_loader::{self};
use crate::config::{Config, ConfigEntry};
use crate::sandbox::Sandbox;
use crate::util::{self, block_on, spawn_blocking, spawn_blocking_detached};
use crate::{config, icc, orientation, ColorState, Error, Image, MimeType, SandboxMechanism};

/// Max texture size 8 GB in bytes
pub(crate) const MAX_TEXTURE_SIZE: u64 = 8 * 10u64.pow(9);

#[derive(Clone, Debug)]
pub struct RemoteProcess<'a, P: ZbusProxy<'a>> {
    _dbus_connection: zbus::Connection,
    decoding_instruction: P,
    mime_type: String,
    phantom: PhantomData<&'a P>,
    pub stderr_content: Arc<Mutex<String>>,
    pub stdout_content: Arc<Mutex<String>>,
    transform_to_memory_format: Option<MemoryFormat>,
}

pub trait ZbusProxy<'a>: Sized + Sync + Send + From<zbus::Proxy<'a>> {
    fn builder(conn: &zbus::Connection) -> zbus::proxy::Builder<'a, Self>;
    fn expose_base_dir(config: &Config, mime_type: &MimeType) -> Result<bool, Error>;
    fn entry_config(config: &Config, mime_type: &MimeType) -> Result<Box<dyn ConfigEntry>, Error>;
}

impl<'a> ZbusProxy<'a> for LoaderProxy<'a> {
    fn builder(conn: &zbus::Connection) -> zbus::proxy::Builder<'a, Self> {
        Self::builder(conn)
    }

    fn expose_base_dir(config: &Config, mime_type: &MimeType) -> Result<bool, Error> {
        Ok(config.loader(mime_type)?.expose_base_dir)
    }

    fn entry_config(config: &Config, mime_type: &MimeType) -> Result<Box<dyn ConfigEntry>, Error> {
        Ok(Box::new(config.loader(mime_type)?.clone()))
    }
}

impl<'a> ZbusProxy<'a> for EditorProxy<'a> {
    fn builder(conn: &zbus::Connection) -> zbus::proxy::Builder<'a, Self> {
        Self::builder(conn)
    }

    fn expose_base_dir(config: &Config, mime_type: &MimeType) -> Result<bool, Error> {
        Ok(config.editor(mime_type)?.expose_base_dir)
    }

    fn entry_config(config: &Config, mime_type: &MimeType) -> Result<Box<dyn ConfigEntry>, Error> {
        Ok(Box::new(config.editor(mime_type)?.clone()))
    }
}

impl<'a, P: ZbusProxy<'a>> RemoteProcess<'a, P> {
    pub async fn new(
        mime_type: &config::MimeType,
        config: &config::Config,
        sandbox_mechanism: SandboxMechanism,
        file: &gio::File,
        cancellable: &gio::Cancellable,
        transform_to_memory_format: Option<MemoryFormat>,
    ) -> Result<Self, Error> {
        // UnixStream which facilitates the D-Bus connection. The stream is passed as
        // stdin to loader binaries.
        let (unix_stream, loader_stdin) = std::os::unix::net::UnixStream::pair()?;
        unix_stream.set_nonblocking(true)?;
        loader_stdin.set_nonblocking(true)?;

        let config_entry = P::entry_config(config, mime_type)?;
        let mut sandbox = Sandbox::new(sandbox_mechanism, config_entry, loader_stdin);
        // Mount dir that contains the file as read only for formats like SVG
        if P::expose_base_dir(config, mime_type)? {
            if let Some(base_dir) = file.parent().and_then(|x| x.path()) {
                sandbox.add_ro_bind(base_dir);
            }
        }
        let spawned_sandbox = sandbox.spawn().await?;
        let mut subprocess = spawned_sandbox.child;
        let command_dbg = spawned_sandbox.info.command_dbg;

        let stderr_content: Arc<Mutex<String>> = Default::default();
        spawn_stdio_reader(&mut subprocess.stderr, &stderr_content, "stderr");

        let stdout_content: Arc<Mutex<String>> = Default::default();
        spawn_stdio_reader(&mut subprocess.stdout, &stdout_content, "stdout");

        #[cfg(feature = "tokio")]
        let unix_stream = tokio::net::UnixStream::from_std(unix_stream)?;

        let guid = zbus::Guid::generate();
        let dbus_result = zbus::connection::Builder::unix_stream(unix_stream)
            .p2p()
            .server(guid)?
            .auth_mechanism(zbus::AuthMechanism::Anonymous)
            .build()
            .shared();

        let subprocess_id = nix::unistd::Pid::from_raw(subprocess.id().try_into().unwrap());

        futures_util::select! {
            _result = dbus_result.clone().fuse() => Ok(()),
            _result = cancellable.future().fuse() => {
                let _result = signal::kill(subprocess_id, signal::Signal::SIGKILL);
                Err(glib::Error::from(gio::Cancelled).into())
            },
            return_status = spawn_blocking(move || subprocess.wait()).fuse() => match return_status {
                Ok(status) => Err(Error::PrematureExit { status, cmd: command_dbg }),
                Err(err) => Err(Error::StdIoError{ err: err.into(), info: command_dbg }),
            }
        }?;

        cancellable.connect_cancelled(move |_| {
            let _result = signal::kill(subprocess_id, signal::Signal::SIGKILL);
        });

        let dbus_connection = dbus_result.await?;

        let decoding_instruction = P::builder(&dbus_connection)
            // Unused since P2P connection
            .destination("org.gnome.glycin")?
            .build()
            .await?;

        Ok(Self {
            _dbus_connection: dbus_connection,
            decoding_instruction,
            mime_type: mime_type.to_string(),
            phantom: PhantomData,
            stderr_content,
            stdout_content,
            transform_to_memory_format,
        })
    }

    fn init_request(
        &self,
        gfile_worker: &GFileWorker,
        base_dir: Option<std::path::PathBuf>,
    ) -> Result<InitRequest, Error> {
        let (remote_reader, writer) = std::os::unix::net::UnixStream::pair()?;

        gfile_worker.write_to(writer)?;

        let fd = zvariant::OwnedFd::from(OwnedFd::from(remote_reader));

        let mime_type = self.mime_type.clone();

        let mut details = InitializationDetails::default();
        details.base_dir = base_dir;

        Ok(InitRequest {
            fd,
            mime_type,
            details,
        })
    }
}

impl<'a> RemoteProcess<'a, LoaderProxy<'a>> {
    pub async fn init(
        &self,
        gfile_worker: GFileWorker,
        base_dir: Option<std::path::PathBuf>,
    ) -> Result<ImageInfo, Error> {
        let init_request = self.init_request(&gfile_worker, base_dir)?;

        let image_info = self.decoding_instruction.init(init_request).shared();

        let reader_error = gfile_worker.error();
        futures_util::pin_mut!(reader_error);

        futures_util::select! {
            _result = image_info.clone().fuse() => Ok(()),
            result = reader_error.fuse() => result,
        }?;

        let image_info = image_info.await?;

        // Seal all memfds
        if let Some(exif) = &image_info.details.exif {
            seal_fd(exif).await?;
        }
        if let Some(xmp) = &image_info.details.xmp {
            seal_fd(xmp).await?;
        }

        Ok(image_info)
    }

    pub async fn request_frame<'b>(
        &self,
        frame_request: FrameRequest,
        image: &Image<'b>,
    ) -> Result<api_loader::Frame, Error> {
        let mut frame = self.decoding_instruction.frame(frame_request).await?;

        // Seal all constant data
        if let Some(iccp) = &frame.details.iccp {
            seal_fd(iccp).await?;
        }

        let raw_fd = frame.texture.as_raw_fd();
        let original_mmap = unsafe { MmapMut::map_mut(raw_fd) }?;

        validate_frame(&frame, &original_mmap)?;

        let img_buf = ImgBuf::MMap(original_mmap);

        let img_buf = if image.loader.apply_transformations {
            orientation::apply_exif_orientation(img_buf, &mut frame, image.info())
        } else {
            img_buf
        };

        let mut color_state = ColorState::Srgb;

        let img_buf = if let Some(cicp) = frame
            .details
            .cicp
            .clone()
            .and_then(|x| x.try_into().ok())
            .and_then(|x| Cicp::from_bytes(&x).ok())
        {
            color_state = ColorState::Cicp(cicp);
            img_buf
        } else if let Some(Ok(icc_profile)) = frame.details.iccp.as_ref().map(|x| x.get()) {
            // Align stride with pixel size if necessary
            let mut img_buf = remove_stride_if_needed(img_buf, raw_fd, &mut frame)?;

            let memory_format = frame.memory_format;
            let (icc_mmap, icc_result) = spawn_blocking(move || {
                let result = icc::apply_transformation(&icc_profile, memory_format, &mut img_buf);
                (img_buf, result)
            })
            .await;

            match icc_result {
                Err(err) => {
                    eprintln!("Failed to apply ICC profile: {err}");
                }
                Ok(new_color_state) => {
                    color_state = new_color_state;
                }
            }

            icc_mmap
        } else {
            img_buf
        };

        let img_buf = if let Some(target_format) = self.transform_to_memory_format {
            glycin_utils::editing::change_memory_format(img_buf, &mut frame, target_format)?
        } else {
            img_buf
        };

        let bytes = match img_buf {
            ImgBuf::MMap(mmap) => {
                drop(mmap);
                seal_fd(raw_fd).await?;
                unsafe { gbytes_from_mmap(raw_fd)? }
            }
            ImgBuf::Vec(vec) => glib::Bytes::from_owned(vec),
        };

        Ok(api_loader::Frame {
            buffer: bytes,
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            memory_format: frame.memory_format,
            delay: frame.delay.into(),
            details: frame.details,
            color_state,
        })
    }
}

impl<'a> RemoteProcess<'a, EditorProxy<'a>> {
    pub async fn editor_apply_sparse(
        &self,
        gfile_worker: &GFileWorker,
        base_dir: Option<std::path::PathBuf>,
        operations: &Operations,
    ) -> Result<SparseEditorOutput, Error> {
        let init_request = self.init_request(gfile_worker, base_dir)?;
        let edit_request = EditRequest::for_operations(operations)?;

        let editor_output = self
            .decoding_instruction
            .apply(init_request, edit_request)
            .shared();

        let reader_error = gfile_worker.error();
        futures_util::pin_mut!(reader_error);

        futures_util::select! {
            _result = editor_output.clone().fuse() => Ok(()),
            result = reader_error.fuse() => result,
        }?;

        let editor_output = editor_output.await?;

        Ok(editor_output)
    }

    pub async fn editor_apply_complete(
        &self,
        gfile_worker: &GFileWorker,
        base_dir: Option<std::path::PathBuf>,
        operations: &Operations,
    ) -> Result<CompleteEditorOutput, Error> {
        let init_request = self.init_request(gfile_worker, base_dir)?;
        let edit_request = EditRequest::for_operations(operations)?;

        let editor_output = self
            .decoding_instruction
            .apply_complete(init_request, edit_request)
            .shared();

        let reader_error = gfile_worker.error();
        futures_util::pin_mut!(reader_error);

        futures_util::select! {
            _result = editor_output.clone().fuse() => Ok(()),
            result = reader_error.fuse() => result,
        }?;

        let editor_output = editor_output.await?;

        Ok(editor_output)
    }
}

use std::io::{BufReader, Write};
const BUF_SIZE: usize = u16::MAX as usize;

#[zbus::proxy(
    interface = "org.gnome.glycin.Loader",
    default_path = "/org/gnome/glycin"
)]
pub trait Loader {
    async fn init(&self, init_request: InitRequest) -> Result<ImageInfo, RemoteError>;
    async fn frame(&self, frame_request: FrameRequest) -> Result<Frame, RemoteError>;
}

#[zbus::proxy(
    interface = "org.gnome.glycin.Editor",
    default_path = "/org/gnome/glycin"
)]
pub trait Editor {
    async fn apply(
        &self,
        init_request: InitRequest,
        edit_request: EditRequest,
    ) -> Result<SparseEditorOutput, RemoteError>;

    async fn apply_complete(
        &self,
        init_request: InitRequest,
        edit_request: EditRequest,
    ) -> Result<CompleteEditorOutput, RemoteError>;
}

pub struct GFileWorker {
    file: gio::File,
    writer_send: Mutex<Option<oneshot::Sender<UnixStream>>>,
    first_bytes_recv: future::Shared<oneshot::Receiver<Arc<Vec<u8>>>>,
    error_recv: future::Shared<oneshot::Receiver<Result<(), Error>>>,
}
use std::sync::Mutex;
impl GFileWorker {
    pub fn spawn(file: gio::File, cancellable: gio::Cancellable) -> GFileWorker {
        let gfile = file.clone();

        let (error_send, error_recv) = oneshot::channel();
        let (first_bytes_send, first_bytes_recv) = oneshot::channel();
        let (writer_send, writer_recv) = oneshot::channel();

        spawn_blocking_detached(move || {
            Self::handle_errors(error_send, move || {
                let reader = gfile.read(Some(&cancellable))?;
                let mut buf = vec![0; BUF_SIZE];

                let n = reader.read(&mut buf, Some(&cancellable))?;
                let first_bytes = Arc::new(buf[..n].to_vec());
                first_bytes_send
                    .send(first_bytes.clone())
                    .or(Err(Error::InternalCommunicationCanceled))?;

                let mut writer: UnixStream = block_on(writer_recv)?;

                writer.write_all(&first_bytes)?;
                drop(first_bytes);

                loop {
                    let n = reader.read(&mut buf, Some(&cancellable))?;
                    if n == 0 {
                        break;
                    }
                    writer.write_all(&buf[..n])?;
                }

                Ok(())
            })
        });

        GFileWorker {
            file,
            writer_send: Mutex::new(Some(writer_send)),
            first_bytes_recv: first_bytes_recv.shared(),
            error_recv: error_recv.shared(),
        }
    }

    fn handle_errors(
        error_send: oneshot::Sender<Result<(), Error>>,
        f: impl FnOnce() -> Result<(), Error>,
    ) {
        let result = f();
        let _result = error_send.send(result);
    }

    pub fn write_to(&self, stream: UnixStream) -> Result<(), Error> {
        let sender = std::mem::take(&mut *self.writer_send.lock().unwrap());

        sender
            // TODO: this fails if write_to is called a second time
            .unwrap()
            .send(stream)
            .or(Err(Error::InternalCommunicationCanceled))
    }

    pub fn file(&self) -> &gio::File {
        &self.file
    }

    pub async fn error(&self) -> Result<(), Error> {
        match self.error_recv.clone().await {
            Ok(result) => result,
            Err(_) => Ok(()),
        }
    }

    pub async fn head(&self) -> Result<Arc<Vec<u8>>, Error> {
        futures_util::select!(
            err = self.error_recv.clone() => err?,
            _bytes = self.first_bytes_recv.clone() => Ok(()),
        )?;

        match self.first_bytes_recv.clone().await {
            Err(_) => self.error_recv.clone().await?.map(|_| Default::default()),
            Ok(bytes) => Ok(bytes),
        }
    }
}

async fn seal_fd(fd: impl AsRawFd) -> Result<(), memfd::Error> {
    let raw_fd = fd.as_raw_fd();

    let start = Instant::now();

    let mfd = memfd::Memfd::try_from_fd(raw_fd).unwrap();
    // In rare circumstances the sealing returns a ResourceBusy
    loop {
        // 🦭
        let seal = mfd.add_seals(&[
            memfd::FileSeal::SealShrink,
            memfd::FileSeal::SealGrow,
            memfd::FileSeal::SealWrite,
            memfd::FileSeal::SealSeal,
        ]);

        match seal {
            Ok(_) => break,
            Err(err) if start.elapsed() > Duration::from_secs(10) => {
                // Give up after some time and return the error
                return Err(err);
            }
            Err(_) => {
                // Try again after short waiting time
                util::sleep(Duration::from_millis(1)).await;
            }
        }
    }
    mem::forget(mfd);

    Ok(())
}

fn validate_frame(frame: &Frame, mmap: &MmapMut) -> Result<(), Error> {
    if mmap.len() < frame.n_bytes()? {
        return Err(Error::TextureTooSmall {
            texture_size: mmap.len(),
            frame: format!("{:?}", frame),
        });
    }

    if frame.stride < frame.width.smul(frame.memory_format.n_bytes().u32())? {
        return Err(Error::StrideTooSmall(format!("{:?}", frame)));
    }

    if frame.width < 1 || frame.height < 1 {
        return Err(Error::WidgthOrHeightZero(format!("{:?}", frame)));
    }

    if (frame.stride as u64).smul(frame.height as u64)? > MAX_TEXTURE_SIZE {
        return Err(Error::TextureTooLarge);
    }

    // Ensure
    frame.width.try_i32()?;
    frame.height.try_i32()?;
    frame.stride.try_usize()?;

    Ok(())
}

unsafe fn gbytes_from_mmap(raw_fd: RawFd) -> Result<glib::Bytes, Error> {
    let mut error = std::ptr::null_mut();

    let mapped_file = glib::ffi::g_mapped_file_new_from_fd(raw_fd, glib::ffi::GFALSE, &mut error);

    if !error.is_null() {
        let err: glib::Error = glib::translate::from_glib_full(error);
        return Err(err.into());
    };

    let bytes = glib::translate::from_glib_full(glib::ffi::g_mapped_file_get_bytes(mapped_file));

    glib::ffi::g_mapped_file_unref(mapped_file);

    Ok(bytes)
}

fn remove_stride_if_needed(
    img_buf: ImgBuf,
    raw_fd: RawFd,
    frame: &mut Frame,
) -> Result<ImgBuf, Error> {
    if frame.stride.srem(frame.memory_format.n_bytes().u32())? == 0 {
        return Ok(img_buf);
    }

    match img_buf {
        ImgBuf::Vec(_) => Ok(img_buf),
        ImgBuf::MMap(mut mmap) => {
            let borrowed_fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw_fd) };

            let width = frame
                .width
                .try_usize()?
                .smul(frame.memory_format.n_bytes().usize())?;
            let stride = frame.stride.try_usize()?;
            let mut source = vec![0; width];
            for row in 1..frame.height.try_usize()? {
                source.copy_from_slice(&mmap[row.smul(stride)?..row.smul(stride)?.sadd(width)?]);
                mmap[row.smul(width)?..row.sadd(1)?.smul(width)?].copy_from_slice(&source);
            }
            frame.stride = width.try_u32()?;

            // This mmap would have the wrong size after ftruncate
            drop(mmap);

            nix::unistd::ftruncate(
                borrowed_fd,
                libc::off_t::try_from(frame.n_bytes()?).map_err(|_| DimensionTooLargerError)?,
            )?;

            // Need a new mmap with correct size
            let mmap = unsafe { memmap::MmapMut::map_mut(raw_fd) }?;
            Ok(ImgBuf::MMap(mmap))
        }
    }
}

fn spawn_stdio_reader(
    stdio: &mut Option<impl Read + Send + 'static>,
    store: &Arc<Mutex<String>>,
    name: &'static str,
) {
    if let Some(stdout) = stdio.take() {
        let store = store.clone();
        util::spawn_blocking_detached(move || {
            let mut stdout = BufReader::new(stdout);

            let mut buf = String::new();
            while let Ok(len) = stdout.read_line(&mut buf) {
                if len == 0 {
                    break;
                }
                tracing::debug!("Loader {name}: {buf}", buf = buf.trim_end());
                store.lock().unwrap().push_str(&buf);
                buf.clear();
            }
        });
    }
}
