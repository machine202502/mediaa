use crossbeam_channel::Sender;
use mediaa_protocol::backend::{PreviewKindWire, PreviewRequestWire};
use std::sync::OnceLock;

static SENDER: OnceLock<Sender<PreviewRequestWire>> = OnceLock::new();

pub fn install(sender: Sender<PreviewRequestWire>) {
    let _ = SENDER.set(sender);
}

pub fn request(path: &std::path::Path) -> bool {
    let Some(sender) = SENDER.get() else {
        return false;
    };
    sender
        .try_send(PreviewRequestWire {
            path: path.to_string_lossy().into_owned(),
            kind: PreviewKindWire::Player,
        })
        .is_ok()
}
