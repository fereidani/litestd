//! A plain enum, for targets whose pointers have no room for both an OS code
//! and a tag.

use alloc_crate::boxed::Box;

use super::{Custom, Form, SimpleMessage};

pub(super) struct Repr(Form<Box<Custom>>);

impl Repr {
    #[inline]
    pub(super) const fn new(form: Form<Box<Custom>>) -> Self {
        Self(form)
    }

    #[inline]
    pub(super) const fn from_message(message: &'static SimpleMessage) -> Self {
        Self(Form::Message(message))
    }

    #[inline]
    pub(super) fn form(&self) -> Form<&Custom> {
        match &self.0 {
            Form::Os(code) => Form::Os(*code),
            Form::Simple(kind) => Form::Simple(*kind),
            Form::Message(message) => Form::Message(message),
            Form::Custom(custom) => Form::Custom(custom),
        }
    }

    #[inline]
    pub(super) fn form_mut(&mut self) -> Form<&mut Custom> {
        match &mut self.0 {
            Form::Os(code) => Form::Os(*code),
            Form::Simple(kind) => Form::Simple(*kind),
            Form::Message(message) => Form::Message(message),
            Form::Custom(custom) => Form::Custom(custom),
        }
    }

    #[inline]
    pub(super) fn into_form(self) -> Form<Box<Custom>> {
        self.0
    }
}
