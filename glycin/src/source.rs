use std::io::Read;
use std::os::unix::net::UnixStream;

use futures_channel::mpsc::TryRecvError;
use futures_util::SinkExt;
use gio::prelude::*;

use crate::util::AsyncWriteExt;
use crate::{Error, Source, util};

const BUF_SIZE: usize = u16::MAX as usize;

#[derive(Debug)]
pub struct SourceTransmission {
    file: Option<gio::File>,
    input_stream: gio::InputStream,
    first_bytes: Vec<u8>,
}

impl SourceTransmission {
    pub async fn init(source: Source) -> Result<SourceTransmission, Error> {
        let input_stream = source.to_stream().await.unwrap();
        let buf = vec![0; BUF_SIZE];

        // TODO: Use cancallable here
        let (buf, n) = input_stream
            .read_future(buf, glib::Priority::DEFAULT)
            .await
            .map_err(|(_, err)| Error::ImageSource(err))
            .unwrap();

        let first_bytes = buf[..n].to_vec();

        Ok(Self {
            file: source.file(),
            input_stream,
            first_bytes,
        })
    }

    /*
    pub fn builtin_reader(self) -> Result<gio::InputStreamRead<gio::InputStream>, Error> {
        if let Some(seekable) = self
            .input_stream_read
            .input_stream()
            .upcast_ref::<glib::Object>()
            .downcast_ref::<gio::Seekable>()
        {
            seekable.seek(0, glib::SeekType::Set, Some(&self.cancellable))?;
        } else {
            todo!()
        }

        Ok(self.input_stream_read)
    }
     */

    async fn spawn_with_stream(self, mut stream: util::UnixStream) -> Result<(), Error> {
        stream.write_all(&self.first_bytes).await?;

        loop {
            let buf = vec![0; BUF_SIZE];

            let (buf, n) = self
                .input_stream
                .read_future(buf, glib::Priority::DEFAULT)
                .await
                .map_err(|(_, err)| Error::ImageSource(err))?;
            if n == 0 {
                return Ok(());
            }

            stream.write_all(&buf[..n]).await?;
        }
    }

    pub fn spawn_external(
        self,
    ) -> Result<(UnixStream, impl Future<Output = Result<(), Error>>), Error> {
        let (external_reader, writer) = std::os::unix::net::UnixStream::pair()?;

        let writer = async_io::Async::new(writer)?;

        Ok((external_reader, self.spawn_with_stream(writer)))
    }

    #[cfg(feature = "builtin")]
    async fn spawn_with_channel(
        self,
        mut channel: futures_channel::mpsc::Sender<Vec<u8>>,
    ) -> Result<(), Error> {
        channel.send(self.first_bytes.to_vec()).await.unwrap();

        loop {
            let buf = vec![0; BUF_SIZE];

            let (buf, n) = self
                .input_stream
                .read_future(buf, glib::Priority::DEFAULT)
                .await
                .map_err(|(_, err)| Error::ImageSource(err))?;
            if n == 0 {
                return Ok(());
            }

            channel.send(buf[..n].to_vec()).await.unwrap();
        }
    }

    #[cfg(feature = "builtin")]
    pub fn spawn_builtin(self) -> (BuiltinSourceReader, impl Future<Output = Result<(), Error>>) {
        let (writer, builtin_reader) = futures_channel::mpsc::channel(100);

        let builtin_reader = BuiltinSourceReader::new(builtin_reader);

        (builtin_reader, self.spawn_with_channel(writer))
    }

    pub fn file(&self) -> Option<&gio::File> {
        self.file.as_ref()
    }

    pub fn first_bytes(&self) -> &[u8] {
        &self.first_bytes
    }
}

#[cfg(feature = "builtin")]
pub struct BuiltinSourceReader {
    stream: futures_channel::mpsc::Receiver<Vec<u8>>,
    cache: Vec<u8>,
}

#[cfg(feature = "builtin")]
impl BuiltinSourceReader {
    fn new(stream: futures_channel::mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            stream,
            cache: Vec::new(),
        }
    }
}

#[cfg(feature = "builtin")]
impl Read for BuiltinSourceReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if !self.cache.is_empty() {
                let (len, cache) = write_data(buf, &self.cache);
                self.cache = cache;
                return Ok(len);
            } else {
                match self.stream.try_recv() {
                    Err(TryRecvError::Closed) => return Ok(0),
                    Err(TryRecvError::Empty) => continue,
                    Ok(data) => {
                        let (len, cache) = write_data(buf, &data);
                        self.cache = cache;
                        return Ok(len);
                    }
                }
            }
        }
    }
}

#[cfg(feature = "builtin")]
fn write_data(target: &mut [u8], src: &[u8]) -> (usize, Vec<u8>) {
    if let Some((send, cache)) = src.split_at_checked(target.len()) {
        target.copy_from_slice(send);
        (target.len(), cache.to_vec())
    } else {
        target[..src.len()].copy_from_slice(&src);
        (src.len(), Vec::new())
    }
}
