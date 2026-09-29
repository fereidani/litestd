//! One pointer-sized word, for 64-bit targets. Its two low bits tag the form:
//!
//! - `00`: a `&'static SimpleMessage`, stored as it is: its alignment keeps the
//!   tag bits clear, so `const_error!` needs no address arithmetic.
//! - `01`: a `Box<Custom>` pointer, tagged and untagged with `map_addr`, which
//!   keeps its provenance.
//! - `10`: an OS error code, in the high 32 bits.
//! - `11`: an `ErrorKind` discriminant, in the high 32 bits.
//!
//! The last two are addresses without provenance, never dereferenced.

use core::{
    marker::PhantomData,
    mem::ManuallyDrop,
    ptr::{self, NonNull},
};

use alloc_crate::boxed::Box;

use super::{Custom, ErrorKind, Form, SimpleMessage};

const TAG_MASK: usize = 0b11;
const TAG_MESSAGE: usize = 0b00;
const TAG_CUSTOM: usize = 0b01;
const TAG_OS: usize = 0b10;
const TAG_SIMPLE: usize = 0b11;

// Both pointer forms leave the tag bits clear.
const _: () = assert!(
    align_of::<SimpleMessage>() > TAG_MASK && align_of::<Custom>() > TAG_MASK
);

/// An error in one word.
///
/// # Safety
///
/// `word` is always as `new` or `from_message` made it, which `decode`
/// relies on, and so holds only `Send` and `Sync` data, which the impls of
/// those traits rely on.
pub(super) struct Repr {
    word: NonNull<()>,
    /// Owning a `Box<Custom>` gives `Repr` std's auto traits, all but `Send`
    /// and `Sync`, which the raw pointer loses.
    owns: PhantomData<Box<Custom>>,
}

// What every form holds is `Send` and `Sync`, as the impls below rely on.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Box<Custom>>();
    send_sync::<&'static SimpleMessage>();
};

// SAFETY: a `Repr` owns a `Box<Custom>` or holds a `&'static SimpleMessage`
// or plain integers, all `Send` as checked above; the word only encodes them.
unsafe impl Send for Repr {}
// SAFETY: as above for `Sync`; `&Repr` gives shared access only.
unsafe impl Sync for Repr {}

impl Repr {
    /// Packs `form`, taking ownership of a custom error's box.
    #[inline]
    pub(super) fn new(form: Form<Box<Custom>>) -> Self {
        let word = match form {
            #[allow(clippy::cast_sign_loss, reason = "the code keeps its bits")]
            Form::Os(code) => with_payload(code as u32, TAG_OS),
            Form::Simple(kind) => with_payload(kind as u32, TAG_SIMPLE),
            Form::Message(message) => return Self::from_message(message),
            Form::Custom(custom) => Box::into_raw(custom)
                .map_addr(|addr| addr | TAG_CUSTOM)
                .cast(),
        };
        // SAFETY: the tag of each of these forms is not zero.
        let word = unsafe { NonNull::new_unchecked(word) };
        Self {
            word,
            owns: PhantomData,
        }
    }

    /// Stores a static message untagged, in a `const` context.
    #[inline]
    pub(super) const fn from_message(message: &'static SimpleMessage) -> Self {
        let message = ptr::from_ref(message).cast_mut().cast();
        // SAFETY: a reference is never null.
        let word = unsafe { NonNull::new_unchecked(message) };
        Self {
            word,
            owns: PhantomData,
        }
    }

    /// Borrows the form.
    #[inline]
    pub(super) fn form(&self) -> Form<&Custom> {
        // SAFETY: `decode` hands over the pointer that `Box::into_raw` gave
        // `new`, so it is aligned and not null, and the box it owns stays
        // alive and unchanged while `&self` lives.
        self.decode(|custom| unsafe { &*custom })
    }

    /// Borrows the form mutably.
    #[inline]
    #[allow(
        clippy::needless_pass_by_ref_mut,
        reason = "the unique borrow of `self` makes the returned one unique"
    )]
    pub(super) fn form_mut(&mut self) -> Form<&mut Custom> {
        // SAFETY: as in `form`, and `&mut self` makes the borrow unique.
        self.decode(|custom| unsafe { &mut *custom })
    }

    /// Unpacks the form, handing over a custom error's box.
    #[inline]
    pub(super) fn into_form(self) -> Form<Box<Custom>> {
        let this = ManuallyDrop::new(self);
        // SAFETY: the pointer came from `Box::into_raw`, and the box is taken
        // once, since `this` is never dropped.
        this.decode(|custom| unsafe { Box::from_raw(custom) })
    }

    /// Decodes the word, turning a custom error's untagged pointer into `C`.
    #[inline]
    fn decode<C>(&self, custom: impl FnOnce(*mut Custom) -> C) -> Form<C> {
        let bits = self.word.addr().get();
        #[allow(clippy::cast_possible_truncation, reason = "the high half")]
        let payload = (bits >> 32) as u32;
        match bits & TAG_MASK {
            TAG_MESSAGE => {
                let message = self.word.cast::<SimpleMessage>();
                // SAFETY: an untagged word is a `&'static SimpleMessage`, see
                // `from_message`.
                Form::Message(unsafe { message.as_ref() })
            }
            TAG_CUSTOM => {
                let untagged = self.word.as_ptr().map_addr(|a| a & !TAG_MASK);
                Form::Custom(custom(untagged.cast()))
            }
            #[allow(clippy::cast_possible_wrap, reason = "the code's bits")]
            TAG_OS => Form::Os(payload as i32),
            _ => Form::Simple(ErrorKind::from_bits(payload)),
        }
    }
}

impl Drop for Repr {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: as in `into_form`; `self` is not used again.
        let form = self.decode(|custom| unsafe { Box::from_raw(custom) });
        if let Form::Custom(custom) = form {
            drop_custom(custom);
        }
    }
}

/// Frees a custom error out of line, so that the drop glue inlined wherever
/// an error is dropped only tests the tag.
#[cold]
#[inline(never)]
fn drop_custom(custom: Box<Custom>) {
    drop(custom);
}

/// A word without provenance holding `payload` above the tag bits.
#[inline]
const fn with_payload(payload: u32, tag: usize) -> *mut () {
    ptr::without_provenance_mut(((payload as usize) << 32) | tag)
}
