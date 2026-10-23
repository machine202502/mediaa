use crate::audio::AudioSink;
use crate::frame::Frame;

pub trait FrameSource {
    fn name(&self) -> String;
    fn size(&self) -> (u32, u32);
    fn grab(&mut self) -> anyhow::Result<Frame>;
}

pub trait AudioSource: Send {
    fn name(&self) -> String;
    fn start(
        &mut self,
        open_sink: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>,
    ) -> anyhow::Result<()>;
    fn stop(&mut self);
}

pub type SourceFactory = Box<dyn FnOnce() -> anyhow::Result<Box<dyn FrameSource>> + Send>;
