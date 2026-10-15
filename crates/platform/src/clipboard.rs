//! Clipboard: images, file lists (paste into Explorer/Finder/chat apps) and text.

use anyhow::anyhow;
use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContext, RustImageData};
use mediaa_core::Frame;
use std::path::Path;

thread_local! {
    // On X11 the clipboard content lives only as long as its owner, so keep one context alive.
    static CTX: std::cell::RefCell<Option<ClipboardContext>> = const { std::cell::RefCell::new(None) };
}

fn with_ctx<R>(f: impl FnOnce(&ClipboardContext) -> anyhow::Result<R>) -> anyhow::Result<R> {
    CTX.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(ClipboardContext::new().map_err(|e| anyhow!("clipboard: {e}"))?);
        }
        f(slot.as_ref().expect("initialised above"))
    })
}

pub fn copy_image(frame: &Frame) -> anyhow::Result<()> {
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())
        .ok_or_else(|| anyhow!("invalid frame"))?;
    let data = RustImageData::from_dynamic_image(image::DynamicImage::ImageRgba8(img));
    with_ctx(|c| c.set_image(data).map_err(|e| anyhow!("clipboard: {e}")))
}

pub fn copy_files(paths: &[&Path]) -> anyhow::Result<()> {
    let list = paths
        .iter()
        .map(|p| std::path::absolute(p).map(|a| a.to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    with_ctx(|c| c.set_files(list).map_err(|e| anyhow!("clipboard: {e}")))
}

pub fn copy_text(text: &str) -> anyhow::Result<()> {
    with_ctx(|c| c.set_text(text.to_owned()).map_err(|e| anyhow!("clipboard: {e}")))
}
