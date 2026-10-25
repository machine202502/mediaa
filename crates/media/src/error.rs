use crate::job;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaErrorKind {
    Cancelled,
    PermissionDenied,
    SourceUnavailable,
    InvalidInput,
    UnsupportedFormat,
    Busy,
    StorageFull,
    Timeout,
    Io,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    Retry,
    ChooseSource,
    FixPermissions,
    FreeSpace,
    Wait,
    UpdateApp,
}

impl MediaErrorKind {
    pub fn requires_attention(self) -> bool {
        !matches!(
            self,
            MediaErrorKind::Cancelled | MediaErrorKind::InvalidInput | MediaErrorKind::Internal
        )
    }

    pub fn recovery_action(self) -> Option<RecoveryAction> {
        match self {
            MediaErrorKind::Cancelled => None,
            MediaErrorKind::PermissionDenied => Some(RecoveryAction::FixPermissions),
            MediaErrorKind::SourceUnavailable | MediaErrorKind::InvalidInput => {
                Some(RecoveryAction::ChooseSource)
            }
            MediaErrorKind::UnsupportedFormat => Some(RecoveryAction::UpdateApp),
            MediaErrorKind::Busy => Some(RecoveryAction::Wait),
            MediaErrorKind::StorageFull => Some(RecoveryAction::FreeSpace),
            MediaErrorKind::Timeout | MediaErrorKind::Io => Some(RecoveryAction::Retry),
            MediaErrorKind::Internal => Some(RecoveryAction::UpdateApp),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct MediaProblem {
    kind: MediaErrorKind,
    message: String,
}

impl MediaProblem {
    pub fn new(kind: MediaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

pub fn invalid_input(message: impl Into<String>) -> anyhow::Error {
    MediaProblem::new(MediaErrorKind::InvalidInput, message).into()
}

pub fn timed_out(message: impl Into<String>) -> anyhow::Error {
    MediaProblem::new(MediaErrorKind::Timeout, message).into()
}

pub fn classify_media_error(error: &anyhow::Error) -> MediaErrorKind {
    if job::is_cancelled(error) {
        return MediaErrorKind::Cancelled;
    }
    if let Some(problem) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<MediaProblem>())
    {
        return problem.kind;
    }
    let io_kind = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind);
    match io_kind {
        Some(
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::IsADirectory,
        ) => MediaErrorKind::SourceUnavailable,
        Some(std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem) => {
            MediaErrorKind::PermissionDenied
        }
        Some(std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded) => {
            MediaErrorKind::StorageFull
        }
        Some(
            std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::ResourceBusy
            | std::io::ErrorKind::ExecutableFileBusy,
        ) => MediaErrorKind::Busy,
        Some(
            std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::InvalidFilename
            | std::io::ErrorKind::UnexpectedEof,
        ) => MediaErrorKind::InvalidInput,
        Some(std::io::ErrorKind::Unsupported | std::io::ErrorKind::NotSeekable) => {
            MediaErrorKind::UnsupportedFormat
        }
        Some(std::io::ErrorKind::TimedOut) => MediaErrorKind::Timeout,
        Some(_) => MediaErrorKind::Io,
        None => MediaErrorKind::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_media_error, invalid_input, timed_out, MediaErrorKind};
    use crate::job::Cancelled;

    #[test]
    fn media_errors_are_classified_without_text_matching() {
        let io = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        assert_eq!(classify_media_error(&io), MediaErrorKind::PermissionDenied);
        assert_eq!(
            classify_media_error(&std::io::Error::from(std::io::ErrorKind::NotFound).into()),
            MediaErrorKind::SourceUnavailable
        );
        assert_eq!(
            classify_media_error(&std::io::Error::from(std::io::ErrorKind::StorageFull).into()),
            MediaErrorKind::StorageFull
        );
        assert_eq!(
            classify_media_error(&std::io::Error::from(std::io::ErrorKind::WouldBlock).into()),
            MediaErrorKind::Busy
        );
        assert_eq!(
            classify_media_error(&invalid_input("bad preset")),
            MediaErrorKind::InvalidInput
        );
        assert_eq!(
            classify_media_error(&timed_out("worker")),
            MediaErrorKind::Timeout
        );
        assert_eq!(
            classify_media_error(&anyhow::Error::new(Cancelled)),
            MediaErrorKind::Cancelled
        );
        assert_eq!(
            classify_media_error(&anyhow::anyhow!("encoder failed")),
            MediaErrorKind::Internal
        );
        assert!(!MediaErrorKind::InvalidInput.requires_attention());
        assert!(MediaErrorKind::StorageFull.requires_attention());
        assert_eq!(
            MediaErrorKind::StorageFull.recovery_action(),
            Some(super::RecoveryAction::FreeSpace)
        );
    }
}
