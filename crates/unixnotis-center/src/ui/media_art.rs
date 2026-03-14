use std::cell::RefCell;
use std::io::Cursor;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gdk_pixbuf::Pixbuf;
use gio::MemoryInputStream;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use image::ImageReader;
use tracing::debug;

use crate::media::MediaArtSource;

// Small budgets keep artwork from becoming a memory or latency sink
const MEDIA_ART_TIMEOUT: Duration = Duration::from_secs(3);
const MEDIA_ART_CHUNK_BYTES: usize = 64 * 1024;
const MAX_LOCAL_ART_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REMOTE_ART_BYTES: usize = 2 * 1024 * 1024;
const MAX_MEDIA_ART_DIMENSION: u32 = 2048;
const MAX_MEDIA_ART_PIXELS: u32 = 4_194_304;

pub(super) fn apply_media_art(
    picture: &gtk::Picture,
    current_key: &Rc<RefCell<Option<String>>>,
    source: Option<&MediaArtSource>,
) {
    let next_key = source.map(MediaArtSource::stable_key);
    // Matching keys mean the same art request is already visible or in flight
    if *current_key.borrow() == next_key {
        return;
    }

    // Update the request key first so stale async completions can be ignored later
    *current_key.borrow_mut() = next_key.clone();
    clear_picture(picture);

    let Some(source) = source.cloned() else {
        picture.add_css_class("empty");
        return;
    };

    // Invalid local files fail fast before any async work is queued
    if matches!(&source, MediaArtSource::LocalFile(path) if !local_art_file_allowed(path)) {
        picture.add_css_class("empty");
        return;
    }

    let picture_weak = picture.downgrade();
    let current_key = current_key.clone();
    glib::MainContext::default().spawn_local(async move {
        let texture = load_art_texture(&source).await;
        // Late completions must not overwrite a newer request
        if *current_key.borrow() != next_key {
            return;
        }
        let Some(picture) = picture_weak.upgrade() else {
            return;
        };
        if let Some(texture) = texture {
            picture.set_paintable(Some(&texture));
            picture.remove_css_class("empty");
            return;
        }
        picture.add_css_class("empty");
    });
}

fn clear_picture(picture: &gtk::Picture) {
    // Clearing both file and paintable avoids mixing old and new loading paths
    picture.set_file(None::<&gio::File>);
    picture.set_paintable(None::<&gdk::Texture>);
}

fn local_art_file_allowed(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    // Only regular files are allowed here to avoid device and fifo edge cases
    metadata.is_file() && metadata.len() <= MAX_LOCAL_ART_BYTES
}

async fn load_art_texture(source: &MediaArtSource) -> Option<gdk::Texture> {
    // The timeout drops the GIO future which cancels the in-flight read
    let bytes = glib::future_with_timeout(MEDIA_ART_TIMEOUT, load_art_bytes(source))
        .await
        .ok()??;
    if !art_dimensions_allowed(&bytes) {
        debug!("media artwork rejected because dimensions were too large");
        return None;
    }
    let bytes = glib::Bytes::from_owned(bytes);
    let stream = MemoryInputStream::from_bytes(&bytes);
    let pixbuf = Pixbuf::from_stream(&stream, None::<&gio::Cancellable>).ok()?;
    Some(gdk::Texture::for_pixbuf(&pixbuf))
}

async fn load_art_bytes(source: &MediaArtSource) -> Option<Vec<u8>> {
    match source {
        MediaArtSource::LocalFile(path) => {
            // Native paths are safer to load through a local file handle
            let file = gio::File::for_path(path);
            read_file_bytes_limited(&file, MAX_LOCAL_ART_BYTES as usize).await
        }
        MediaArtSource::RemoteHttps(url) => {
            // Remote art still goes through the same byte cap as local art
            let file = gio::File::for_uri(url.as_str());
            read_file_bytes_limited(&file, MAX_REMOTE_ART_BYTES).await
        }
    }
}

async fn read_file_bytes_limited(file: &gio::File, max_bytes: usize) -> Option<Vec<u8>> {
    let stream = file.read_future(glib::Priority::default()).await.ok()?;
    let mut out = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(MEDIA_ART_CHUNK_BYTES, glib::Priority::default())
            .await
            .ok()?;
        if chunk.is_empty() {
            break;
        }
        let bytes = chunk.as_ref();
        if out.len().saturating_add(bytes.len()) > max_bytes {
            debug!(
                max_bytes,
                "media artwork rejected because it exceeded the byte cap"
            );
            return None;
        }
        out.extend_from_slice(bytes);
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

fn art_dimensions_allowed(bytes: &[u8]) -> bool {
    let cursor = Cursor::new(bytes);
    let Ok(reader) = ImageReader::new(cursor).with_guessed_format() else {
        return false;
    };
    // Dimension checks happen before GTK decodes the full image into memory
    let Ok((width, height)) = reader.into_dimensions() else {
        return false;
    };
    if width == 0 || height == 0 {
        return false;
    }
    if width > MAX_MEDIA_ART_DIMENSION || height > MAX_MEDIA_ART_DIMENSION {
        return false;
    }
    width.saturating_mul(height) <= MAX_MEDIA_ART_PIXELS
}

#[cfg(test)]
mod tests {
    use super::art_dimensions_allowed;

    #[test]
    fn art_dimensions_allowed_rejects_non_images() {
        assert!(!art_dimensions_allowed(b"not-an-image"));
    }
}
