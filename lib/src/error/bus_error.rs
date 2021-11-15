use error_chain::error_chain;

error_chain! {
    types {
        BusError, BusErrorKind, ResultExt, Result;
    }
    links {
        LoadError(super::LoadError, super::LoadErrorKind);
        SerializationError(super::SerializationError, super::SerializationErrorKind);
        LockError(super::LockError, super::LockErrorKind);
        TransformError(super::TransformError, super::TransformErrorKind);
    }
    errors {
        ReceiveError(err: String) {
            description("failed to receive event from bus due to an internal error"),
            display("failed to receive event from bus due to an internal error: '{}'", err),
        }
        ChannelClosed {
            description("failed to receive event from bus as the channel is closed"),
            display("failed to receive event from bus as the channel is closed"),
        }
        SaveParentFirst {
            description("you must save the parent object before attempting to initiate a bus from this vector"),
            display("you must save the parent object before attempting to initiate a bus from this vector"),
        }
        WeakDio {
            description("the dio that created this object has gone out of scope"),
            display("the dio that created this object has gone out of scope"),
        }
    }
}
