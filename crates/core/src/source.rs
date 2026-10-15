//! Capture source abstractions. Platform code (screen/window grabbers, microphones) implements
//! these; the recording pipeline only sees the traits, which keeps it testable with fakes.

use crate::audio::AudioSink;
use crate::frame::Frame;

/// Produces RGBA frames of a fixed size (monitor, region or a single window).
///
/// Not `Send`: OS capture handles often are not. Sources are created on the capture thread
/// through a [`SourceFactory`].
pub trait FrameSource {
    /// Human-readable name used in output file names ("Display 1", a window title…).
    fn name(&self) -> String;
    /// Output size; every frame returned by [`grab`](Self::grab) must match it.
    fn size(&self) -> (u32, u32);
    fn grab(&mut self) -> anyhow::Result<Frame>;
}

/// Delivers audio samples into an [`AudioSink`] until stopped.
pub trait AudioSource: Send {
    fn name(&self) -> String;
    /// Starts capturing. `open_sink(sample_rate, channels)` creates the sink once the device
    /// format is known.
    fn start(
        &mut self,
        open_sink: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>,
    ) -> anyhow::Result<()>;
    /// Stops capturing and drops every sink clone it holds.
    fn stop(&mut self);
}

/// Creates a [`FrameSource`] on the thread that will use it.
pub type SourceFactory = Box<dyn FnOnce() -> anyhow::Result<Box<dyn FrameSource>> + Send>;
