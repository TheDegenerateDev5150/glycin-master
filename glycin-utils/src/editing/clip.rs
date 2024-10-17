use std::io::{Cursor, Read, Seek};

use super::{Error, SimpleFrame};

pub fn clip(
    buf: Vec<u8>,
    frame: &mut SimpleFrame,
    (x, y, width, height): (u32, u32, u32, u32),
) -> Result<Vec<u8>, Error> {
    let pixel_size = frame.memory_format.n_bytes().u32();
    let new_stride = width * pixel_size;
    let size = height * new_stride;
    let mut new = Vec::with_capacity(size as usize);

    let stride = frame.stride as i64;
    let x_ = x as i64 * pixel_size as i64;
    let width_ = width as i64 * pixel_size as i64;

    let mut cur = Cursor::new(buf);
    let mut row = vec![0; (width * pixel_size) as usize];

    cur.seek_relative(y as i64 * stride)?;

    for _ in 0..height {
        cur.seek_relative(x_)?;
        cur.read_exact(&mut row)?;
        new.extend_from_slice(&row);
        cur.seek_relative(stride - x_ - width_)?;
    }

    frame.width = width;
    frame.height = height;
    frame.stride = new_stride;

    Ok(new)
}
