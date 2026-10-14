// SPDX-License-Identifier: MPL-2.0

//! The Compact product host over whisper.cpp.

use std::panic::AssertUnwindSafe;

use koru_voice::{EngineError, EngineInfo, ErrorCode, SpeechEngine};
use koru_voice_models::LoadedModel;
use koru_voice_whisper::engine::{WhisperConfig, WhisperEngine};

use crate::engine::EngineHost;

pub const DEFAULT_THREADS: i32 = 4;

#[derive(Clone, Debug)]
pub struct WhisperEngineHost {
    num_threads: i32,
}

impl WhisperEngineHost {
    pub fn new() -> WhisperEngineHost {
        WhisperEngineHost {
            num_threads: DEFAULT_THREADS,
        }
    }

    pub fn with_threads(num_threads: i32) -> WhisperEngineHost {
        WhisperEngineHost {
            num_threads: num_threads.max(1),
        }
    }
}

impl Default for WhisperEngineHost {
    fn default() -> WhisperEngineHost {
        WhisperEngineHost::new()
    }
}

impl EngineHost for WhisperEngineHost {
    fn describe(&self) -> EngineInfo {
        EngineInfo {
            engine: koru_voice_whisper::engine_id(),
            version: "whisper.cpp (no model loaded)".into(),
            model: None,
            languages: Vec::new(),
            streaming: false,
            detects_language: true,
        }
    }

    fn open(&self, model: &LoadedModel) -> Result<Box<dyn SpeechEngine>, EngineError> {
        let model_dir = model.root().to_path_buf();
        let config = WhisperConfig {
            model_dir: model_dir.clone(),
            model: model.installed().id().clone(),
            num_threads: self.num_threads,
            languages: model.descriptor().languages.clone(),
            partial_every: None,
        };
        let built = std::panic::catch_unwind(AssertUnwindSafe(|| WhisperEngine::open(config)));
        match built {
            Ok(Ok(engine)) => Ok(Box::new(engine)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(ErrorCode::ModelLoadFailed.with(format!(
                "the whisper.cpp runtime panicked while loading the model in {}",
                model_dir.display()
            ))),
        }
    }
}
