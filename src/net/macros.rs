//! The methods and `Read` and `Write` impls that the socket types of `net`
//! and `os::unix::net` share. Each type keeps a `sys::net::Socket` in a field
//! named `inner`.

/// Expands, inside an inherent `impl` block, to the methods of the groups
/// named: `common` (`try_clone`, `take_error` and `set_nonblocking`),
/// `timeouts`, `ttl` and `shutdown`.
macro_rules! socket_methods {
    (@common) => {
        /// Creates a new independently owned handle to the underlying socket.
        ///
        /// # Errors
        ///
        /// Fails if the process has no descriptors left.
        pub fn try_clone(&self) -> $crate::io::Result<Self> {
            self.inner.duplicate().map(|inner| Self { inner })
        }

        /// Gets and clears the value of the `SO_ERROR` option on this socket.
        ///
        /// # Errors
        ///
        /// Fails if the option cannot be read; the stored error is the `Ok`
        /// value.
        pub fn take_error(
            &self,
        ) -> $crate::io::Result<Option<$crate::io::Error>> {
            self.inner.take_error()
        }

        /// Moves this socket into or out of nonblocking mode, in which calls
        /// that would block fail with
        /// [`WouldBlock`](crate::io::ErrorKind::WouldBlock).
        ///
        /// # Errors
        ///
        /// Returns the error the OS reports.
        pub fn set_nonblocking(
            &self,
            nonblocking: bool,
        ) -> $crate::io::Result<()> {
            self.inner.set_nonblocking(nonblocking)
        }
    };

    (@timeouts) => {
        /// Sets the read timeout; `None` blocks reads indefinitely. A read
        /// that times out fails with `WouldBlock` on Unix and `TimedOut` on
        /// Windows.
        ///
        /// # Errors
        ///
        /// Fails with [`InvalidInput`](crate::io::ErrorKind::InvalidInput)
        /// for a zero duration.
        pub fn set_read_timeout(
            &self,
            dur: Option<$crate::time::Duration>,
        ) -> $crate::io::Result<()> {
            self.inner.set_read_timeout(dur)
        }

        /// Sets the write timeout; `None` blocks writes indefinitely. A write
        /// that times out fails with `WouldBlock` on Unix and `TimedOut` on
        /// Windows.
        ///
        /// # Errors
        ///
        /// Fails with [`InvalidInput`](crate::io::ErrorKind::InvalidInput)
        /// for a zero duration.
        pub fn set_write_timeout(
            &self,
            dur: Option<$crate::time::Duration>,
        ) -> $crate::io::Result<()> {
            self.inner.set_write_timeout(dur)
        }

        /// Returns the read timeout of this socket.
        ///
        /// # Errors
        ///
        /// Returns the error the OS reports.
        pub fn read_timeout(
            &self,
        ) -> $crate::io::Result<Option<$crate::time::Duration>> {
            self.inner.read_timeout()
        }

        /// Returns the write timeout of this socket.
        ///
        /// # Errors
        ///
        /// Returns the error the OS reports.
        pub fn write_timeout(
            &self,
        ) -> $crate::io::Result<Option<$crate::time::Duration>> {
            self.inner.write_timeout()
        }
    };

    (@ttl) => {
        /// Sets the value for the `IP_TTL` option on this socket.
        ///
        /// # Errors
        ///
        /// Fails with [`InvalidInput`](crate::io::ErrorKind::InvalidInput)
        /// for a value above 255.
        pub fn set_ttl(&self, ttl: u32) -> $crate::io::Result<()> {
            self.inner.set_ttl(ttl)
        }

        /// Gets the value of the `IP_TTL` option for this socket.
        ///
        /// # Errors
        ///
        /// Returns the error the OS reports.
        pub fn ttl(&self) -> $crate::io::Result<u32> {
            self.inner.ttl()
        }
    };

    (@shutdown) => {
        /// Shuts down the read, write, or both halves of this connection.
        ///
        /// # Errors
        ///
        /// Fails with [`NotConnected`](crate::io::ErrorKind::NotConnected) if
        /// not connected.
        pub fn shutdown(
            &self,
            how: $crate::net::Shutdown,
        ) -> $crate::io::Result<()> {
            self.inner.shutdown(how)
        }
    };

    ($($group:ident)*) => {$(
        socket_methods!(@$group);
    )*};
}

pub(crate) use socket_methods;

/// Implements `Read` and `Write` for each type given. A stream lists both
/// `T` and `&T`, as a socket works through a shared reference. Flushing does
/// nothing, as nothing is buffered.
macro_rules! impl_stream_io {
    ($($t:ty),*) => {$(
        impl $crate::io::Read for $t {
            #[inline]
            fn read(&mut self, buf: &mut [u8]) -> $crate::io::Result<usize> {
                self.inner.read(buf)
            }

            #[inline]
            fn read_vectored(
                &mut self,
                bufs: &mut [$crate::io::IoSliceMut<'_>],
            ) -> $crate::io::Result<usize> {
                self.inner.read_vectored(bufs)
            }

            #[inline]
            fn read_to_end(
                &mut self,
                buf: &mut $crate::vec::Vec<u8>,
            ) -> $crate::io::Result<usize> {
                self.inner.read_to_end(buf)
            }

            #[inline]
            fn read_to_string(
                &mut self,
                buf: &mut $crate::string::String,
            ) -> $crate::io::Result<usize> {
                self.inner.read_to_string(buf)
            }
        }

        impl $crate::io::Write for $t {
            #[inline]
            fn write(&mut self, buf: &[u8]) -> $crate::io::Result<usize> {
                self.inner.write(buf)
            }

            #[inline]
            fn write_vectored(
                &mut self,
                bufs: &[$crate::io::IoSlice<'_>],
            ) -> $crate::io::Result<usize> {
                self.inner.write_vectored(bufs)
            }

            #[inline]
            fn flush(&mut self) -> $crate::io::Result<()> {
                Ok(())
            }
        }
    )*};
}

pub(crate) use impl_stream_io;
