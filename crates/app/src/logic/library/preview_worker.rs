use std::path::PathBuf;

pub(crate) fn ensure_hover_proxy(proxy_path: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("hover-proxy".into())
        .spawn(move || {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            if let Err(error) =
                mediaa_media::preview_cache::ensure_hover_proxy(&proxy_path, &cancel)
            {
                log::debug!("hover proxy {}: {error:#}", proxy_path.display());
            }
        });
}
