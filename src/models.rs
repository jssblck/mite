use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::ModelConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub detector_path: PathBuf,
    pub detector_exists: bool,
    pub recognizer_path: PathBuf,
    pub recognizer_exists: bool,
    pub charset_path: Option<PathBuf>,
    pub charset_exists: Option<bool>,
}

impl ModelStatus {
    pub fn inspect(config: &ModelConfig) -> Self {
        Self {
            detector_path: config.detector_path.clone(),
            detector_exists: config.detector_path.exists(),
            recognizer_path: config.recognizer_path.clone(),
            recognizer_exists: config.recognizer_path.exists(),
            charset_path: config.charset_path.clone(),
            charset_exists: config.charset_path.as_ref().map(|path| path.exists()),
        }
    }

    pub fn ready_for_real_inference(&self) -> bool {
        self.detector_exists && self.recognizer_exists && self.charset_exists.unwrap_or(true)
    }

    pub fn missing_paths(&self) -> Vec<&Path> {
        let mut missing = Vec::new();
        if !self.detector_exists {
            missing.push(self.detector_path.as_path());
        }
        if !self.recognizer_exists {
            missing.push(self.recognizer_path.as_path());
        }
        if let (Some(path), Some(false)) = (&self.charset_path, self.charset_exists) {
            missing.push(path.as_path());
        }
        missing
    }
}
