use unixnotis_core::program_in_path;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(super) enum SoundBackend {
    // Event-name and file playback via libcanberra helper
    Canberra,
    // PipeWire file playback fallback
    PwPlay,
    // PulseAudio file playback fallback
    PaPlay,
    // No supported playback command found
    None,
}

pub(super) fn detect_backend() -> SoundBackend {
    // Prefer canberra first because it supports both sound names and files
    if program_in_path("canberra-gtk-play") {
        return SoundBackend::Canberra;
    }
    if program_in_path("pw-play") {
        return SoundBackend::PwPlay;
    }
    if program_in_path("paplay") {
        return SoundBackend::PaPlay;
    }
    SoundBackend::None
}
