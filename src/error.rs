use thiserror::Error;

/// Domain error types for the Hikvision relay app.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("HCNetSDK runtime error: {0}")]
    Sdk(String),

    #[error("HCNetSDK error {code}: {context}")]
    SdkCode { code: u32, context: String },

    #[error("RTSP server error: {0}")]
    Rtsp(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type AppResult<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn sdk_code(&self) -> Option<u32> {
        match self {
            AppError::SdkCode { code, .. } => Some(*code),
            _ => None,
        }
    }
}
