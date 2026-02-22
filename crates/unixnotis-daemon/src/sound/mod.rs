//! Notification sound playback and backend selection

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use unixnotis_core::Config;
use zbus::zvariant::OwnedValue;

mod backend;
mod command;
mod resolve;

use backend::{detect_backend, SoundBackend};
use command::{play_with_canberra, play_with_paplay, play_with_pw_play};
use resolve::{hint_bool, resolve_default_file, resolve_hint_sound};

/// Sound handling for notification playback
pub struct SoundSettings {
    // Global on/off from config
    enabled: bool,
    // Detected backend that is safe to call on this machine
    backend: SoundBackend,
    // Fallback event name used by canberra-style backends
    default_name: Option<String>,
    // Fallback audio file path when hint does not supply one
    default_file: Option<PathBuf>,
    // Last successful play request used for burst throttling
    last_played: Mutex<Option<Instant>>,
}

#[derive(Debug, Clone)]
pub(super) enum SoundSource {
    Name(String),
    File(PathBuf),
}

impl SoundSettings {
    /// Build sound settings from configuration and resolve any custom paths
    pub fn from_config(config: &Config) -> Self {
        // Backend discovery is done once during startup to avoid repeated PATH scans
        let backend = detect_backend();
        debug!(?backend, "sound backend selected");
        if config.sound.enabled && backend == SoundBackend::None {
            warn!("sound enabled but no playback backend found in PATH");
        }

        // Resolve config paths once so notification hot paths stay cheap
        let default_file = resolve_default_file(config);
        Self {
            enabled: config.sound.enabled,
            backend,
            default_name: config.sound.default_name.clone(),
            default_file,
            last_played: Mutex::new(None),
        }
    }

    /// Return true when sound playback is enabled and a backend is available
    pub fn supports_sound(&self) -> bool {
        self.enabled && self.backend != SoundBackend::None
    }

    /// Resolve a sound source from hints or defaults and play if allowed
    pub fn play_from_hints(&self, hints: &HashMap<String, OwnedValue>, allow_sound: bool) {
        // Hard gates first to keep the common no-sound path fast
        if !self.enabled || !allow_sound {
            return;
        }
        // Per-notification hint can force silence even when daemon sound is enabled
        if hint_bool(hints, "suppress-sound").unwrap_or(false) {
            return;
        }
        // Small cooldown avoids noisy bursts when apps spam fast updates
        if !self.should_play_now() {
            return;
        }

        // Hint source wins, then fallback source from config
        let source = resolve_hint_sound(hints).or_else(|| self.default_source());
        if let Some(source) = source {
            self.play(source);
        }
    }

    fn default_source(&self) -> Option<SoundSource> {
        if let Some(path) = self.default_file.as_ref() {
            // File fallback is tried before event-name fallback
            return Some(SoundSource::File(path.clone()));
        }
        self.default_name
            .as_ref()
            .map(|name| SoundSource::Name(name.clone()))
    }

    fn play(&self, source: SoundSource) {
        // Backend-specific launcher keeps this method tiny and testable
        match self.backend {
            SoundBackend::Canberra => play_with_canberra(source),
            SoundBackend::PwPlay => play_with_pw_play(source),
            SoundBackend::PaPlay => play_with_paplay(source),
            SoundBackend::None => {}
        }
    }

    fn should_play_now(&self) -> bool {
        const MIN_INTERVAL: Duration = Duration::from_millis(150);
        let Ok(mut guard) = self.last_played.lock() else {
            // A poisoned lock should not disable alerts forever
            return true;
        };
        let now = Instant::now();
        if let Some(last) = *guard {
            // Skip playback if requests are too close together
            if now.duration_since(last) < MIN_INTERVAL {
                return false;
            }
        }
        // Record now only when the request is accepted
        *guard = Some(now);
        true
    }
}
