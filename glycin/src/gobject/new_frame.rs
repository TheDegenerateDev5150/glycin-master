use gio::glib;
use glib::prelude::*;
use glib::subclass::prelude::*;
use glycin_utils::MemoryFormat;

use std::sync::OnceLock;

use super::init;

static_assertions::assert_impl_all!(GlyNewFrame: Send, Sync);

pub mod imp {

    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::GlyNewFrame)]
    pub struct GlyNewFrame {
        #[property(get, construct_only)]
        width: OnceLock<u32>,
        #[property(get, construct_only)]
        height: OnceLock<u32>,
        #[property(get, construct_only)]
        stride: OnceLock<u32>,
        #[property(get, construct_only, builder(MemoryFormat::R8g8b8))]
        memory_format: OnceLock<MemoryFormat>,
        #[property(get, construct_only)]
        texture: OnceLock<glib::Bytes>,

        #[property(get, set, nullable)]
        color_icc_profile: Mutex<Option<glib::Bytes>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlyNewFrame {
        const NAME: &'static str = "GlyNewFrame";
        type Type = super::GlyNewFrame;
    }

    #[glib::derived_properties]
    impl ObjectImpl for GlyNewFrame {
        fn constructed(&self) {
            self.parent_constructed();

            init();
        }
    }
}

glib::wrapper! {
    /// GObject wrapper for [`Loader`]
    pub struct GlyNewFrame(ObjectSubclass<imp::GlyNewFrame>);
}

impl GlyNewFrame {
    pub fn new(
        width: u32,
        height: u32,
        stride: Option<u32>,
        memory_format: MemoryFormat,
        texture: glib::Bytes,
    ) -> Self {
        glib::Object::builder()
            .property("width", width)
            .property("height", height)
            .property("stride", stride.unwrap_or_default())
            .property("memory-format", memory_format)
            .property("texture", texture)
            .build()
    }

    pub async fn build(&self, creator: &mut crate::Creator) -> Result<(), crate::Error> {
        let frame = creator.add_frame(
            self.width(),
            self.height(),
            self.memory_format(),
            self.texture().into_data().to_vec(),
        )?;

        frame.set_color_icc_profile(self.color_icc_profile().map(|x| x.into_data().to_vec()))?;

        Ok(())
    }
}
