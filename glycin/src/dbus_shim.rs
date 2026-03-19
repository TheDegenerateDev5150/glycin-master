use std::marker::PhantomData;

pub struct EditorProxy<'a> {
    x: PhantomData<&'a ()>,
}
pub struct LoaderProxy<'a> {
    x: PhantomData<&'a ()>,
}
