//! Checks of speech calls: offsets and speaker counts.

use crate::engine::EngineError;

/// An offset into a recording: seconds, finite, not negative.
pub(crate) fn offset(offset: f64) -> Result<f64, EngineError> {
    if !offset.is_finite() || offset < 0.0 {
        return Err(EngineError::bad_request("offset must be a non-negative number of seconds"));
    }
    Ok(offset)
}

/// How many speakers diarization was told about: a known count is at least one.
pub(crate) fn speakers(speakers: Option<u32>) -> Result<Option<u32>, EngineError> {
    if speakers == Some(0) {
        return Err(EngineError::bad_request("speakers must be greater than 0 when known"));
    }
    Ok(speakers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_speaker_counts_are_checked() {
        assert_eq!(offset(12.5).unwrap(), 12.5);
        assert_eq!(offset(0.0).unwrap(), 0.0);
        assert_eq!(offset(-1.0).unwrap_err().kind, "bad_request");
        assert_eq!(offset(f64::NAN).unwrap_err().kind, "bad_request");
        assert_eq!(speakers(None).unwrap(), None);
        assert_eq!(speakers(Some(2)).unwrap(), Some(2));
        assert_eq!(speakers(Some(0)).unwrap_err().kind, "bad_request");
    }
}
