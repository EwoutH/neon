//! Idiomatic Rust wrappers for N-API threadsafe functions

use std::ffi::c_void;
use std::mem::MaybeUninit;

use crate::napi::bindings as napi;
use crate::raw::{Local, Env};

unsafe fn string(env: Env, s: impl AsRef<str>) -> Local {
    let s = s.as_ref();
    let mut result = MaybeUninit::uninit();

    assert_eq!(
        napi::create_string_utf8(
            env,
            s.as_bytes().as_ptr() as *const _,
            s.len(),
            result.as_mut_ptr(),
        ),
        napi::Status::Ok,
    );

    result.assume_init()
}

#[derive(Debug)]
struct Tsfn(napi::ThreadsafeFunction);

unsafe impl Send for Tsfn {}
unsafe impl Sync for Tsfn {}

#[derive(Debug)]
/// Threadsafe Function encapsulate a Rust function pointer and N-API threadsafe
/// function for scheduling tasks to execute on a JavaScript thread.
pub struct ThreadsafeFunction<T> {
    tsfn: Tsfn,
    callback: fn(Env, T),
}

#[derive(Debug)]
struct Callback<T> {
    callback: fn(Env, T),
    data: T,
}

/// Error returned when scheduling a threadsafe function with some data
pub struct CallError<T> {
    kind: napi::Status,
    data: T,
}

impl<T> CallError<T> {
    /// The specific error that occurred
    pub fn kind(&self) -> napi::Status {
        self.kind
    }

    /// Returns the data that was sent when scheduling to allow re-scheduling
    pub fn into_inner(self) -> T {
        self.data
    }
}

impl<T: Send + 'static> ThreadsafeFunction<T> {
    /// Creates a new unbounded N-API Threadsafe Function
    /// Safety: `Env` must be valid for the current thread
    pub unsafe fn new(
        env: Env,
        callback: fn(Env, T),        
    ) -> Self {
        Self::with_capacity(env, 0, callback)
    }

    /// Creates a bounded N-API Threadsafe Function
    /// Safety: `Env` must be valid for the current thread
    pub unsafe fn with_capacity(
        env: Env,
        max_queue_size: usize,
        callback: fn(Env, T),
    ) -> Self {
        let mut result = MaybeUninit::uninit();

        assert_eq!(
            napi::create_threadsafe_function(
                env,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                string(env, "neon threadsafe function"),
                max_queue_size,
                // Always set the reference count to 1. Prefer using
                // Rust `Arc` to maintain the struct.
                1,
                std::ptr::null_mut(),
                None,
                std::ptr::null_mut(),
                Some(Self::callback),
                result.as_mut_ptr(),
            ),
            napi::Status::Ok,
        );
    
        Self {
            tsfn: Tsfn(result.assume_init()),
            callback,
        }
    }

    /// Schedule a threadsafe function to be executed with some data
    pub fn call(
        &self,
        data: T,
        is_blocking: Option<napi::ThreadsafeFunctionCallMode>,
    ) -> Result<(), CallError<T>> {
        let is_blocking = is_blocking
            .unwrap_or(napi::ThreadsafeFunctionCallMode::Blocking);

        let callback = Box::into_raw(Box::new(Callback {
            callback: self.callback,
            data,
        }));

        let status = unsafe {
            napi::call_threadsafe_function(
                self.tsfn.0,
                callback as *mut _,
                is_blocking,
            )
        };

        if status == napi::Status::Ok {
            Ok(())
        } else {
            // If the call failed, the callback won't execute
            let callback = unsafe { Box::from_raw(callback) };

            Err(CallError {
                kind: status,
                data: callback.data,
            })
        }
    }

    /// References a threadsafe function to prevent exiting the event loop until it has been dropped. (Default)
    /// Safety: `Env` must be valid for the current thread
    pub unsafe fn reference(&mut self, env: Env) {
        assert_eq!(
            napi::ref_threadsafe_function(
                env,
                self.tsfn.0,
            ),
            napi::Status::Ok,
        );
    }

    /// Unreferences a threadsafe function to allow exiting the event loop before it has been dropped.
    /// Safety: `Env` must be valid for the current thread
    pub unsafe fn unref(&mut self, env: Env) {
        assert_eq!(
            napi::unref_threadsafe_function(
                env,
                self.tsfn.0,
            ),
            napi::Status::Ok,
        );
    }

    // Provides a C ABI wrapper for invoking the user supplied function pointer
    unsafe extern "C" fn callback(
        env: Env,
        _js_callback: napi::Value,
        _context: *mut c_void,
        data: *mut c_void,
    ) {
        // Event loop may be terminated
        if data.is_null() {
            return;
        }

        let Callback {
            callback,
            data,
        } = *Box::from_raw(data as *mut Callback<T>);

        // Event loop has terminated
        if env.is_null() {
            return;
        }

        callback(env, data);
    }
}

impl<T> Drop for ThreadsafeFunction<T> {
    fn drop(&mut self) {
        unsafe {
            napi::release_threadsafe_function(
                self.tsfn.0,
                napi::ThreadsafeFunctionReleaseMode::Release,
            );
        };
    }
}
