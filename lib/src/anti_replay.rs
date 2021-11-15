#![allow(unused_imports)]
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::crypto::AteHash;
use fxhash::FxHashSet;

use super::lint::EventMetadataLinter;
use super::sink::EventSink;
use super::transform::EventDataTransformer;
use super::validator::EventValidator;

use super::error::*;
use super::event::*;
use super::loader::*;
use super::plugin::*;
use super::transaction::ConversationSession;
use super::validator::ValidationResult;

#[derive(Debug, Default, Clone)]
pub struct AntiReplayPlugin {
    seen: FxHashSet<AteHash>,
}

impl AntiReplayPlugin {
    pub fn new() -> AntiReplayPlugin {
        AntiReplayPlugin {
            seen: FxHashSet::default(),
        }
    }

    pub fn push(&mut self, hash: AteHash) {
        self.seen.insert(hash);
    }
}

impl EventSink for AntiReplayPlugin {
    fn feed(
        &mut self,
        header: &EventHeader,
        _conversation: Option<&Arc<ConversationSession>>,
    ) -> Result<(), SinkError> {
        self.seen.insert(header.raw.event_hash);
        Ok(())
    }

    fn reset(&mut self) {
        self.seen.clear();
    }
}

impl EventValidator for AntiReplayPlugin {
    fn validate(
        &self,
        header: &EventHeader,
        _conversation: Option<&Arc<ConversationSession>>,
    ) -> Result<ValidationResult, ValidationError> {
        match self.seen.contains(&header.raw.event_hash) {
            true => {
                #[cfg(feature = "enable_verbose")]
                debug!(
                    "rejected event as it is a duplicate - {}",
                    header.raw.event_hash
                );
                Ok(ValidationResult::Deny)
            }
            false => Ok(ValidationResult::Abstain),
        }
    }

    fn clone_validator(&self) -> Box<dyn EventValidator> {
        Box::new(self.clone())
    }

    fn validator_name(&self) -> &str {
        "anti-reply-validator"
    }
}

impl EventMetadataLinter for AntiReplayPlugin {
    fn clone_linter(&self) -> Box<dyn EventMetadataLinter> {
        Box::new(self.clone())
    }
}

impl EventDataTransformer for AntiReplayPlugin {
    fn clone_transformer(&self) -> Box<dyn EventDataTransformer> {
        Box::new(self.clone())
    }
}

impl EventPlugin for AntiReplayPlugin {
    fn clone_plugin(&self) -> Box<dyn EventPlugin> {
        Box::new(self.clone())
    }
}

impl Loader for AntiReplayPlugin {
    fn relevance_check(&mut self, header: &EventData) -> bool {
        match header.as_header_raw() {
            Ok(a) => {
                let ret = self.seen.contains(&a.event_hash);
                self.seen.insert(a.event_hash);
                ret
            }
            _ => false,
        }
    }
}
