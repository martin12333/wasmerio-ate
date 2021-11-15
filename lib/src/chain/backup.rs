#[allow(unused_imports)]
use tracing::{debug, error, info, instrument, span, trace, warn, Level};

use crate::error::*;

use super::*;

impl<'a> Chain {
    pub async fn backup(&'a self, include_active_files: bool) -> Result<(), SerializationError> {
        let delayed_operations = {
            let mut single = self.single().await;
            single
                .inside_async
                .chain
                .redo
                .backup(include_active_files)?
        };
        delayed_operations.await?;
        Ok(())
    }
}
