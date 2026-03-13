use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

use crate::{Error, util};

pub async fn seal_fd(fd: impl AsRawFd) -> Result<(), memfd::Error> {
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
    std::mem::forget(mfd);

    Ok(())
}
