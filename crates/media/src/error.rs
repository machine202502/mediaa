use crate::job;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaErrorKind {
    Cancelled,
    Io,
    Internal,
}

pub fn classify_media_error(error: &anyhow::Error) -> MediaErrorKind {
    if job::is_cancelled(error) {
        return MediaErrorKind::Cancelled;
    }
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
    {
        return MediaErrorKind::Io;
    }
    MediaErrorKind::Internal
}

#[cfg(test)]
mod tests {
    use super::{classify_media_error, MediaErrorKind};
    use crate::job::Cancelled;

    #[test]
    fn media_errors_are_classified_without_text_matching() {
        let io = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        assert_eq!(classify_media_error(&io), MediaErrorKind::Io);
        assert_eq!(
            classify_media_error(&anyhow::Error::new(Cancelled)),
            MediaErrorKind::Cancelled
        );
        assert_eq!(
            classify_media_error(&anyhow::anyhow!("encoder failed")),
            MediaErrorKind::Internal
        );
    }
}
