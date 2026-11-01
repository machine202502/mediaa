pub(crate) fn audio_out() -> Result<mediaa_platform::audio::AudioOut, String> {
    mediaa_platform::audio::AudioOut::open().map_err(|error| format!("{error:#}"))
}
