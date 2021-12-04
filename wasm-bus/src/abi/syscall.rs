#![allow(dead_code)]
use super::*;
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};

mod raw {
    use super::*;

    // Frees memory that was passed to the operating system by
    // the program
    #[no_mangle]
    pub extern "C" fn wasm_bus_free(buf_ptr: u32, buf_len: u32) {
        trace!("wasm_bus_free (buf={} bytes)", buf_len);
        unsafe {
            let data = Vec::from_raw_parts(buf_ptr as *mut u8, buf_len as usize, buf_len as usize);
            std::mem::drop(data);
        }
    }

    // Allocates memory that will be used to pass data from the
    // operating system back to this program
    #[no_mangle]
    pub extern "C" fn wasm_bus_malloc(len: u32) -> u32 {
        trace!("wasm_bus_malloc (len={})", len);
        let mut buf = Vec::with_capacity(len as usize);
        let ptr: *mut u8 = buf.as_mut_ptr();
        std::mem::forget(buf);
        return ptr as u32;
    }

    // Invoked by the operating system when during a poll when a
    // request is to be processed by this program
    #[no_mangle]
    pub extern "C" fn wasm_bus_start(
        handle: u32,
        topic_ptr: u32,
        topic_len: u32,
        request_ptr: u32,
        request_len: u32,
    ) {
        let topic = unsafe {
            let topic =
                Vec::from_raw_parts(topic_ptr as *mut u8, topic_len as usize, topic_len as usize);
            String::from_utf8_lossy(&topic[..]).to_string()
        };
        trace!(
            "wasm_bus_start (handle={}, topic={}, request={} bytes)",
            handle,
            topic,
            request_len
        );
        unsafe {
            let request = Vec::from_raw_parts(
                request_ptr as *mut u8,
                request_len as usize,
                request_len as usize,
            );

            let handle: CallHandle = handle.into();
            if let Err(err) = crate::engine::BusEngine::start(topic, handle, request) {
                fault(handle.into(), err as u32);
            }
        }

        #[cfg(feature = "rt")]
        crate::task::wake();
    }

    // Invoked by the operating system when a call has finished
    // This call includes the data that was returned
    #[no_mangle]
    pub extern "C" fn wasm_bus_finish(handle: u32, data: u32, data_len: u32) {
        trace!(
            "wasm_bus_finish (handle={}, response={} bytes)",
            handle,
            data_len
        );
        unsafe {
            let response =
                Vec::from_raw_parts(data as *mut u8, data_len as usize, data_len as usize);

            crate::engine::BusEngine::finish(handle.into(), response);
        }

        #[cfg(feature = "rt")]
        crate::task::wake();
    }

    // Invoked by the operating system when the call this program made has failed
    #[no_mangle]
    pub extern "C" fn wasm_bus_error(handle: u32, error: u32) {
        trace!("wasm_bus_err (handle={}, error={})", handle, error);
        crate::engine::BusEngine::error(handle.into(), error.into());

        #[cfg(feature = "rt")]
        crate::task::wake();
    }

    #[link(wasm_import_module = "wasm-bus")]
    extern "C" {
        // Returns a handle 32-bit number which is used while generating
        // handles for calls and receive hooks
        pub(crate) fn rand() -> u32;

        // Indicates that a fault has occured while processing a call
        pub(crate) fn fault(handle: u32, error: u32);

        // Returns the response of a listen invokation to a program
        // from the operating system
        pub(crate) fn reply(handle: u32, response: i32, response_len: i32);

        // Drops a handle used by calls or callbacks
        pub(crate) fn drop(handle: u32);

        // Calls a function using the operating system call to find
        // the right target based on the wapm and topic.
        // The operating system will respond with either a 'wasm_bus_finish'
        // or a 'wasm_bus_error' message.
        pub(crate) fn call(
            parent: u32,
            handle: u32,
            wapm: i32,
            wapm_len: i32,
            topic: i32,
            topic_len: i32,
            request: i32,
            request_len: i32,
        ) -> u32;

        // Incidates that a call that will be made should invoke a callback
        // back to this process under the designated handle.
        pub(crate) fn callback(parent: u32, handle: u32, topic: i32, topic_len: i32);

        // Tells the operating system that this program is ready to respond
        // to calls on a particular topic name.
        pub(crate) fn listen(topic: i32, topic_len: i32);

        // Polls the operating system for messages which will be returned via
        // the 'wasm_bus_start' function call.
        pub(crate) fn poll();

        // Returns a unqiue ID for the thread
        pub(crate) fn thread_id() -> u32;
    }
}

pub fn drop(handle: CallHandle) {
    unsafe { raw::drop(handle.id) }
}

pub fn rand() -> u32 {
    unsafe { raw::rand() }
}

pub fn fault(handle: CallHandle, error: u32) {
    unsafe {
        raw::fault(handle.id, error);
    }
}

pub fn poll() {
    unsafe { raw::poll() }
}

pub fn listen(topic: &str) {
    unsafe {
        let topic_len = topic.len();
        let topic = topic.as_ptr();
        raw::listen(topic as i32, topic_len as i32)
    }
}

pub fn reply(handle: CallHandle, response: &[u8]) {
    unsafe {
        let response_len = response.len();
        let response = response.as_ptr();
        raw::reply(handle.id, response as i32, response_len as i32);
    }
}

pub fn call(
    parent: Option<CallHandle>,
    handle: CallHandle,
    wapm: &str,
    topic: &str,
    request: &[u8],
) {
    let ret = unsafe {
        let parent = parent.map(|a| a.id).unwrap_or_else(|| u32::MAX);
        let wapm_len = wapm.len();
        let wapm = wapm.as_ptr();
        let topic_len = topic.len();
        let topic = topic.as_ptr();
        let request_len = request.len();
        let request = request.as_ptr();
        raw::call(
            parent,
            handle.id,
            wapm as i32,
            wapm_len as i32,
            topic as i32,
            topic_len as i32,
            request as i32,
            request_len as i32,
        )
    };

    if CallError::Success as u32 != ret {
        raw::wasm_bus_error(handle.id, ret);
    }
}

pub fn callback(parent: CallHandle, handle: CallHandle, topic: &str) {
    unsafe {
        let topic_len = topic.len();
        let topic = topic.as_ptr();
        raw::callback(parent.id, handle.id, topic as i32, topic_len as i32)
    }
}

pub fn thread_id() -> u32 {
    unsafe { raw::thread_id() }
}
