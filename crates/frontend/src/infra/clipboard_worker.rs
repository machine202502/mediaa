use crossbeam_channel::{Receiver, Sender};
use mediaa_core::Frame;
use mediaa_protocol::backend::ClipboardWire;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Purpose {
    Toast(String),
    Recording { success: String, sound: bool },
    Silent,
}

pub struct Event {
    pub purpose: Purpose,
    pub result: Result<(), String>,
}

pub struct ClipboardHandle {
    tx: Sender<ClipboardWire>,
}

impl Clone for ClipboardHandle {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl ClipboardHandle {
    pub fn copy_text(&self, text: String, _purpose: Purpose) -> anyhow::Result<()> {
        self.send(ClipboardWire {
            text: Some(text),
            files: Vec::new(),
            image: None,
        })
    }

    pub fn copy_files(&self, files: Vec<PathBuf>, _purpose: Purpose) -> anyhow::Result<()> {
        self.send(ClipboardWire {
            text: None,
            files: files
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            image: None,
        })
    }

    pub fn copy_image_wait(&self, frame: Frame) -> Result<(), String> {
        let dir = std::env::temp_dir().join("mediaa-clipboard");
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let path = dir.join(format!(
            "clip-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.data)
            .ok_or_else(|| "bad image buffer".to_string())?;
        image.save(&path).map_err(|error| error.to_string())?;
        self.send(ClipboardWire {
            text: None,
            files: Vec::new(),
            image: Some(path.to_string_lossy().into_owned()),
        })
        .map_err(|error| format!("{error:#}"))
    }

    fn send(&self, wire: ClipboardWire) -> anyhow::Result<()> {
        self.tx
            .try_send(wire)
            .map_err(|_| anyhow::anyhow!("clipboard queue is full"))
    }
}

pub struct ClipboardService {
    tx: Sender<ClipboardWire>,
    rx: Option<Receiver<ClipboardWire>>,
}

impl ClipboardService {
    pub fn new(_wake: mediaa_core::wake::Wake) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self { tx, rx: Some(rx) }
    }

    #[cfg(test)]
    pub fn noop(_wake: mediaa_core::wake::Wake) -> Self {
        Self::new(_wake)
    }

    pub fn handle(&mut self) -> ClipboardHandle {
        ClipboardHandle {
            tx: self.tx.clone(),
        }
    }

    pub fn take_commands(&mut self) -> Option<Receiver<ClipboardWire>> {
        self.rx.take()
    }

    pub fn try_recv(&self) -> Option<Event> {
        None
    }

    pub fn is_busy(&self) -> bool {
        false
    }
}
