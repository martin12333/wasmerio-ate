use error_chain::error_chain;
use rmp_serde::decode::Error as RmpDecodeError;
use rmp_serde::encode::Error as RmpEncodeError;

use crate::crypto::AteHash;
use crate::header::PrimaryKey;

error_chain! {
    types {
        LoadError, LoadErrorKind, ResultExt, Result;
    }
    links {
        SerializationError(super::SerializationError, super::SerializationErrorKind);
        TransformationError(super::TransformError, super::TransformErrorKind);
    }
    errors {
        IO(err: String) {
            description("IO error")
            display("{}", err)
        }
        NotFound(key: PrimaryKey) {
            description("data object with key could not be found"),
            display("data object with key ({}) could not be found", key.as_hex_string()),
        }
        NoPrimaryKey {
            description("entry has no primary could and hence could not be loaded")
            display("entry has no primary could and hence could not be loaded")
        }
        VersionMismatch {
            description("entry has an invalid version for this log file")
            display("entry has an invalid version for this log file")
        }
        NotFoundByHash(hash: AteHash) {
            description("data object with hash could not be found"),
            display("data object with hash ({}) could not be found", hash.to_string()),
        }
        ObjectStillLocked(key: PrimaryKey) {
            description("data object with key is still being edited in the current scope"),
            display("data object with key ({}) is still being edited in the current scope", key.as_hex_string()),
        }
        AlreadyDeleted(key: PrimaryKey) {
            description("data object with key has already been deleted"),
            display("data object with key ({}) has already been deleted", key.as_hex_string()),
        }
        Tombstoned(key: PrimaryKey) {
            description("data object with key has already been tombstoned"),
            display("data object with key ({}) has already been tombstoned", key.as_hex_string()),
        }
        ChainCreationError(err: String) {
            description("chain creation error while attempting to load data object"),
            display("chain creation error while attempting to load data object - {}", err),
        }
        NoRepository {
            description("chain has no repository thus could not load foreign object")
            display("chain has no repository thus could not load foreign object")
        }
        MissingData {
            description("the data is missing for this record")
            display("the data is missing for this record")
        }
        Disconnected {
            description("unable to load record as the client is currently disconnected from the server")
            display("unable to load record as the client is currently disconnected from the server")
        }
        Timeout {
            description("timeout while waiting for the data from the server")
            display("timeout while waiting for the data from the server")
        }
        LoadFailed(err: String) {
            description("failed to load the data from the server"),
            display("failed to load the data from the server - {}", err),
        }
        CollectionDetached {
            description("collection is detached from its parent, it must be attached before it can be used")
            display("collection is detached from its parent, it must be attached before it can be used")
        }
        WeakDio {
            description("the dio that created this object has gone out of scope")
            display("the dio that created this object has gone out of scope")
        }
    }
}

impl From<tokio::io::Error> for LoadError {
    fn from(err: tokio::io::Error) -> LoadError {
        LoadErrorKind::IO(err.to_string()).into()
    }
}

impl From<RmpEncodeError> for LoadError {
    fn from(err: RmpEncodeError) -> LoadError {
        LoadErrorKind::SerializationError(super::SerializationErrorKind::EncodeError(err).into())
            .into()
    }
}

impl From<RmpDecodeError> for LoadError {
    fn from(err: RmpDecodeError) -> LoadError {
        LoadErrorKind::SerializationError(super::SerializationErrorKind::DecodeError(err).into())
            .into()
    }
}

impl From<bincode::Error> for LoadError {
    fn from(err: bincode::Error) -> LoadError {
        LoadErrorKind::SerializationError(super::SerializationErrorKind::BincodeError(err).into())
            .into()
    }
}

impl From<super::ChainCreationError> for LoadError {
    fn from(err: super::ChainCreationError) -> LoadError {
        LoadErrorKind::ChainCreationError(err.to_string()).into()
    }
}

impl From<super::ChainCreationErrorKind> for LoadError {
    fn from(err: super::ChainCreationErrorKind) -> LoadError {
        LoadErrorKind::ChainCreationError(err.to_string()).into()
    }
}
