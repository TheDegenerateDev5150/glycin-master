use glycin_utils::{Frame, FungibleMemory};

use crate::Image;

pub fn apply_exif_orientation(
    frame: Frame<FungibleMemory>,
    image: &Image,
) -> Frame<FungibleMemory> {
    if image.details().transformation_ignore_exif() {
        frame
    } else {
        let orientation = image.transformation_orientation();
        glycin_utils::editing::change_orientation(frame, orientation)
    }
}
