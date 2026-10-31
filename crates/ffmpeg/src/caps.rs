pub fn encoder_available(name: &str) -> bool {
    ez_ffmpeg::capabilities::is_encoder_available(name)
}

pub fn filter_available(name: &str) -> bool {
    ez_ffmpeg::capabilities::is_filter_available(name)
}
