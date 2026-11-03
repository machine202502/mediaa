use crate::ffmpeg::input::Input;
use crate::ffmpeg::output::Output;
use crate::ffmpeg::push::PushSource;
use ez_ffmpeg::core::scheduler::ffmpeg_scheduler::{FfmpegScheduler, Running};
use ez_ffmpeg::{FfmpegContext, Input as EzInput};

pub struct JobContext {
    inner: FfmpegContext,
}

pub struct Job {
    inner: FfmpegScheduler<Running>,
}

#[derive(Default)]
pub struct JobBuilder {
    inputs: Vec<EzInput>,
    filter: Option<String>,
    output: Option<ez_ffmpeg::Output>,
}

impl JobBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input(mut self, input: Input) -> Self {
        self.inputs.push(input.into_inner());
        self
    }

    pub fn push_input(mut self, source: PushSource) -> Self {
        self.inputs.push(EzInput::from(source.into_inner()));
        self
    }

    pub fn filter_desc(mut self, graph: &str) -> Self {
        self.filter = Some(graph.to_string());
        self
    }

    pub fn output(mut self, output: Output) -> Self {
        self.output = Some(output.into_inner());
        self
    }

    pub fn build(self) -> anyhow::Result<JobContext> {
        let output = self
            .output
            .ok_or_else(|| anyhow::anyhow!("FFmpeg job has no output"))?;
        let mut builder = FfmpegContext::builder().inputs(self.inputs);
        if let Some(filter) = self.filter {
            builder = builder.filter_desc(filter);
        }
        Ok(JobContext {
            inner: builder.output(output).build()?,
        })
    }
}

impl JobContext {
    pub fn start(self) -> anyhow::Result<Job> {
        Ok(Job {
            inner: self.inner.start()?,
        })
    }
}

impl Job {
    pub fn is_ended(&self) -> bool {
        self.inner.is_ended()
    }

    pub fn wait(self) -> anyhow::Result<()> {
        self.inner.wait().map_err(Into::into)
    }

    pub fn abort(self) {
        self.inner.abort();
    }

    pub(crate) fn output_time_us(&self) -> Option<i64> {
        self.inner
            .progress_handle()
            .snapshot()
            .outputs()
            .iter()
            .filter_map(|output| output.out_time_us())
            .max()
    }
}
