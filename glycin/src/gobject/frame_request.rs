use std::marker::PhantomData;
use std::sync::Mutex;

use gio::glib;
use glib::prelude::*;
use glib::subclass::prelude::*;

use crate::FrameRequest;

static_assertions::assert_impl_all!(GlyFrameRequest: Send, Sync);

pub mod imp {

    use super::*;

    #[derive(Default, Debug, glib::Properties)]
    #[properties(wrapper_type = super::GlyFrameRequest)]
    pub struct GlyFrameRequest {
        #[property(get = Self::scale_width)]
        pub scale_width: PhantomData<u32>,
        #[property(get = Self::scale_height)]
        pub scale_height: PhantomData<u32>,

        pub(super) scale: Mutex<Option<(u32, u32)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GlyFrameRequest {
        const NAME: &'static str = "GlyFrameRequest";
        type Type = super::GlyFrameRequest;
    }

    #[glib::derived_properties]
    impl ObjectImpl for GlyFrameRequest {}

    impl GlyFrameRequest {
        fn scale_width(&self) -> u32 {
            self.scale.lock().unwrap().map_or(0, |x| x.0)
        }

        fn scale_height(&self) -> u32 {
            self.scale.lock().unwrap().map_or(0, |x| x.1)
        }
    }
}

glib::wrapper! {
    /// GObject wrapper for [`Loader`]
    pub struct GlyFrameRequest(ObjectSubclass<imp::GlyFrameRequest>);
}

impl GlyFrameRequest {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_scale(&self, width: u32, height: u32) {
        *self.imp().scale.lock().unwrap() = Some((width, height));
    }

    pub fn frame_request(&self) -> FrameRequest {
        let frame_request = FrameRequest::default();

        let frame_request = if let Some((width, height)) = *self.imp().scale.lock().unwrap() {
            frame_request.scale(width, height)
        } else {
            frame_request
        };

        frame_request
    }
}
