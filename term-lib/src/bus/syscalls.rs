use crate::wasmer::Array;
use crate::wasmer::WasmPtr;
use std::collections::HashMap;
use std::future::Future;
use std::task::Context;
use std::task::Poll;
#[allow(unused_imports, dead_code)]
use tracing::{debug, error, info, trace, warn};
use wasm_bus::abi::CallError;
use wasm_bus::abi::CallHandle;

use super::thread::WasmBusThread;
use super::*;

pub(crate) mod raw {
    use super::*;
    pub fn wasm_bus_drop(thread: &WasmBusThread, handle: u32) {
        unsafe { super::wasm_bus_drop(thread, handle) }
    }
    pub fn wasm_bus_rand(thread: &WasmBusThread) -> u32 {
        unsafe { super::wasm_bus_rand(thread) }
    }
    pub fn wasm_bus_tick(thread: &WasmBusThread) -> bool {
        unsafe { super::wasm_bus_tick(thread) }
    }
    pub fn wasm_bus_listen(thread: &WasmBusThread, topic_ptr: WasmPtr<u8, Array>, topic_len: u32) {
        unsafe { super::wasm_bus_listen(thread, topic_ptr, topic_len) }
    }
    pub fn wasm_bus_callback(
        thread: &WasmBusThread,
        parent: u32,
        handle: u32,
        topic_ptr: WasmPtr<u8, Array>,
        topic_len: u32,
    ) {
        unsafe { super::wasm_bus_callback(thread, parent, handle, topic_ptr, topic_len) }
    }
    pub fn wasm_bus_fault(thread: &WasmBusThread, handle: u32, error: u32) {
        unsafe { super::wasm_bus_fault(thread, handle, error) }
    }
    pub fn wasm_bus_poll(thread: &WasmBusThread) {
        unsafe { super::wasm_bus_poll(thread) }
    }
    pub fn wasm_bus_reply(
        thread: &WasmBusThread,
        handle: u32,
        response_ptr: WasmPtr<u8, Array>,
        response_len: u32,
    ) {
        unsafe { super::wasm_bus_reply(thread, handle, response_ptr, response_len) }
    }
    pub fn wasm_bus_call(
        thread: &WasmBusThread,
        parent: u32,
        handle: u32,
        wapm_ptr: WasmPtr<u8, Array>,
        wapm_len: u32,
        topic_ptr: WasmPtr<u8, Array>,
        topic_len: u32,
        request_ptr: WasmPtr<u8, Array>,
        request_len: u32,
    ) -> u32 {
        unsafe {
            super::wasm_bus_call(
                thread,
                parent,
                handle,
                wapm_ptr,
                wapm_len,
                topic_ptr,
                topic_len,
                request_ptr,
                request_len,
            )
        }
    }
    pub fn wasm_bus_thread_id(thread: &WasmBusThread) -> u32 {
        unsafe { super::wasm_bus_thread_id(thread) }
    }
}

// Drops a handle used by calls or callbacks
unsafe fn wasm_bus_drop(thread: &WasmBusThread, handle: u32) {
    let mut inner = thread.inner.unwrap();
    inner.invocations.remove(&handle);
    inner.callbacks.remove(&handle);
    inner.factory.close(CallHandle::from(handle));
}

unsafe fn wasm_bus_rand(_thread: &WasmBusThread) -> u32 {
    fastrand::u32(..)
}

unsafe fn wasm_bus_tick(thread: &WasmBusThread) -> bool
{
    // Take the invocations out of the idle list and process them
    // (we need to do this outside of the thread local lock as
    //  otherwise the re-entrance will panic the system)
    let invocations = {
        let mut inner = thread.inner.unwrap();
        inner.invocations.drain().collect::<Vec<_>>()
    };

    // Run all the invocations and build a carry over list
    let waker = dummy_waker::dummy_waker();
    let mut cx = Context::from_waker(&waker);
    let mut carry_over = Vec::new();
    for (key, mut invocation) in invocations {
        let pinned_invocation = invocation.as_mut();
        if let Poll::Pending = pinned_invocation.poll(&mut cx) {
            carry_over.push((key, invocation));
        }
    }

    // If there are any carry overs then re-add them
    if carry_over.is_empty() == false {
        let mut inner = thread.inner.unwrap();
        for (key, invoke) in carry_over {
            inner.invocations.insert(key, invoke);
        }
    }
}

// Incidates that a call that will be made should invoke a callback
// back to this process under the designated handle.
unsafe fn wasm_bus_callback(
    thread: &WasmBusThread,
    parent: u32,
    handle: u32,
    topic_ptr: WasmPtr<u8, Array>,
    topic_len: u32,
) {
    let parent = if parent != u32::MAX {
        Some(CallHandle::from(parent))
    } else {
        None
    };
    let topic = topic_ptr.get_utf8_str(thread.memory(), topic_len).unwrap();
    debug!(
        "wasm-bus::recv (parent={:?}, handle={}, topic={})",
        parent, handle, topic
    );

    let mut inner = thread.inner.unwrap();
    if let Some(parent) = parent {
        let entry = inner.callbacks.entry(parent.id).or_default();
        entry.insert(topic.to_string(), handle);
        return;
    }
}

// Polls the operating system for messages which will be returned via
// the 'wasm_bus_start' function call.
unsafe fn wasm_bus_poll(thread: &WasmBusThread) {
    debug!("wasm-bus::poll");

    wasm_bus_tick(thread);
    std::thread::sleep(std::time::Duration::from_millis(10));
}

// Tells the operating system that this program is ready to respond
// to calls on a particular topic name.
unsafe fn wasm_bus_listen(thread: &WasmBusThread, topic_ptr: WasmPtr<u8, Array>, topic_len: u32) {
    let topic = topic_ptr.get_utf8_str(thread.memory(), topic_len).unwrap();
    debug!("wasm-bus::listen (topic={})", topic);

    let mut inner = thread.inner.unwrap();
    inner.listens.insert(topic.to_string());
}

// Indicates that a fault has occured while processing a call
unsafe fn wasm_bus_fault(_thread: &WasmBusThread, handle: u32, error: u32) {
    debug!("wasm-bus::error (handle={}, error={})", handle, error);
}

// Returns the response of a listen invokation to a program
// from the operating system
unsafe fn wasm_bus_reply(
    thread: &WasmBusThread,
    handle: u32,
    response_ptr: WasmPtr<u8, Array>,
    response_len: u32,
) {
    debug!(
        "wasm-bus::reply (handle={}, response={} bytes)",
        handle, response_len
    );

    // Grab the data we are sending back
    let _response = thread
        .memory()
        .uint8view_with_byte_offset_and_length(response_ptr.offset(), response_len)
        .to_vec();
}

// Calls a function using the operating system call to find
// the right target based on the wapm and topic.
// The operating system will respond with either a 'wasm_bus_finish'
// or a 'wasm_bus_error' message.
unsafe fn wasm_bus_call(
    thread: &WasmBusThread,
    parent: u32,
    handle: u32,
    wapm_ptr: WasmPtr<u8, Array>,
    wapm_len: u32,
    topic_ptr: WasmPtr<u8, Array>,
    topic_len: u32,
    request_ptr: WasmPtr<u8, Array>,
    request_len: u32,
) -> u32 {
    let parent = if parent != u32::MAX {
        Some(CallHandle::from(parent))
    } else {
        None
    };
    let wapm = wapm_ptr.get_utf8_str(thread.memory(), wapm_len).unwrap();
    let topic = topic_ptr.get_utf8_str(thread.memory(), topic_len).unwrap();
    if let Some(parent) = parent {
        let parent: u32 = parent.into();
        debug!(
            "wasm-bus::call (parent={}, handle={}, wapm={}, topic={}, request={} bytes)",
            parent, handle, wapm, topic, request_len
        );
    } else {
        debug!(
            "wasm-bus::call (handle={}, wapm={}, topic={}, request={} bytes)",
            handle, wapm, topic, request_len
        );
    }

    let request = thread
        .memory()
        .uint8view_with_byte_offset_and_length(request_ptr.offset(), request_len)
        .to_vec();

    // Grab references to the ABI that will be used
    let data_feeder = match WasmBusCallback::new(thread, handle) {
        Ok(a) => a,
        Err(err) => {
            return err.into();
        }
    };

    // Grab all the client callbacks that have been registered
    let client_callbacks: HashMap<String, WasmBusCallback> = thread
        .inner
        .unwrap()
        .callbacks
        .remove(&handle)
        .map(|a| {
            a.into_iter()
                .map(|(topic, handle)| (topic, WasmBusCallback::new(thread, handle).unwrap()))
                .collect()
        })
        .unwrap_or_default();

    // If its got a parent then we already have an active stream here so we need
    // to feed these results into that stream
    let mut invoke = if let Some(parent) = parent {
        if let Some(session) = thread.inner.unwrap().factory.get(parent) {
            session.call(topic.as_ref(), &request)
        } else {
            ErrornousInvokable::new(CallError::InvalidHandle)
        }
    } else {
        thread.inner.unwrap().factory.start(
            handle.into(),
            &wapm,
            &topic,
            &request,
            client_callbacks,
        )
    };

    // Invoke the send operation
    let invoke = {
        let thread = thread.clone();
        async move {
            let response = invoke.process().await;
            thread
                .inner
                .unwrap()
                .factory
                .close(CallHandle::from(handle));
            data_feeder.feed_bytes_or_error(response);
        }
    };

    // We try to invoke the callback synchronously but if it
    // does not complete in time then we add it to the idle
    // processing list which will pick it up again the next time
    // the WASM process yields CPU execution.
    let waker = dummy_waker::dummy_waker();
    let mut cx = Context::from_waker(&waker);
    let mut invoke = Box::pin(invoke);
    if let Poll::Pending = invoke.as_mut().poll(&mut cx) {
        let mut inner = thread.inner.unwrap();
        inner.invocations.insert(handle, invoke);
    }

    // Success
    CallError::Success.into()
}

// Returns a unqiue ID for the thread
unsafe fn wasm_bus_thread_id(thread: &WasmBusThread) -> u32 {
    trace!("wasm-bus::thread_id (id={})", thread.thread_id);
    thread.thread_id
}
