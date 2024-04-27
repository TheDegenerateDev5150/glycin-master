use gdk::{gio, glib};
use gio::ffi::GAsyncReadyCallback;
use gio::prelude::*;
use glib::ffi::gpointer;

struct GPointerSend(pub gpointer);

unsafe impl Send for GPointerSend {}

pub struct GAsyncReadyCallbackSend {
    callback: unsafe extern "C" fn(
        *mut glib::gobject_ffi::GObject,
        *mut gio::ffi::GAsyncResult,
        gpointer,
    ),
    user_data: GPointerSend,
}

unsafe impl Send for GAsyncReadyCallbackSend {}

impl GAsyncReadyCallbackSend {
    pub fn new(callback: GAsyncReadyCallback, user_data: gpointer) -> Self {
        Self {
            callback: callback.unwrap(),
            user_data: GPointerSend(user_data),
        }
    }

    pub unsafe fn call<'a, P, O>(&self, obj: &'a O, res: *mut gio::ffi::GAsyncResult)
    where
        O: glib::translate::ToGlibPtr<'a, *mut P> + IsA<glib::Object>,
    {
        let obj_ptr: *mut P = obj.to_glib_none().0;
        (self.callback)(obj_ptr as *mut _, res, self.user_data.0)
    }
}
