//! Archive read and write helpers for `.unixnotis` bundles
//!
//! The user-facing preset file stays branded as `.unixnotis`
//! The archive details stay here so the rest of the code only deals with
//! validated bundle files and relative preset paths

use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder, Header};

use super::filesystem::CollectedConfigFiles;
use super::manifest::{PresetManifest, PRESET_FORMAT_VERSION};
use super::pathing::{
    archive_payload_path, archive_payload_relative, format_relative_path, MANIFEST_ARCHIVE_PATH,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BundleFile {
    // Relative path inside the UnixNotis config root
    pub(super) relative_path: PathBuf,
    // Full file bytes captured from the bundle
    pub(super) contents: Vec<u8>,
    // Stored mode is restored on import so scripts keep execute bits
    pub(super) mode: u32,
}

#[derive(Debug)]
pub(super) struct BundleArchive {
    // Manifest is loaded first so inspect and import can trust one source of truth
    pub(super) manifest: PresetManifest,
    // Payload files are kept separate from manifest metadata for simpler validation
    pub(super) files: Vec<BundleFile>,
}

pub(super) fn write_bundle(
    bundle_path: &Path,
    manifest: &PresetManifest,
    collected: &CollectedConfigFiles,
) -> Result<()> {
    if let Some(parent) = bundle_path.parent() {
        // Export can target nested output paths, so create the parent tree first
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create preset parent directory {}", parent.display()))?;
    }

    // The file is written once through a compressed tar stream
    let output = File::create(bundle_path)
        .with_context(|| format!("create preset bundle {}", bundle_path.display()))?;
    let encoder = GzEncoder::new(output, Compression::default());
    let mut builder = Builder::new(encoder);

    // Manifest always goes in first so a partial or broken bundle is easy to spot
    let manifest_bytes = manifest.encode()?.into_bytes();
    append_bytes(
        &mut builder,
        Path::new(MANIFEST_ARCHIVE_PATH),
        &manifest_bytes,
        0o644,
    )?;

    for file in &collected.files {
        // Files are streamed from disk so export memory stays bounded by one file at a time
        builder
            .append_path_with_name(&file.source_path, archive_payload_path(&file.relative_path))
            .with_context(|| format!("append {} to archive", file.source_path.display()))?;
    }

    // Finish the tar writer first, then flush the gzip stream to disk
    builder.finish().context("finish preset archive")?;
    let encoder = builder
        .into_inner()
        .context("flush preset archive writer")?;
    encoder.finish().context("finalize preset bundle")?;
    Ok(())
}

pub(super) fn read_bundle(bundle_path: &Path) -> Result<BundleArchive> {
    // Import and inspect use the same reader so validation stays consistent
    let input = File::open(bundle_path)
        .with_context(|| format!("open preset bundle {}", bundle_path.display()))?;
    let decoder = GzDecoder::new(input);
    let mut archive = Archive::new(decoder);

    let mut manifest_contents = None::<String>;
    let mut files = BTreeMap::<PathBuf, BundleFile>::new();

    for entry in archive.entries().context("read preset bundle entries")? {
        let mut entry = entry.context("read preset bundle entry")?;
        if entry.header().entry_type().is_dir() {
            // Tar archives can carry directory records, but preset logic only cares about files
            continue;
        }

        let archive_path = entry.path().context("read bundle entry path")?.into_owned();

        if archive_path == Path::new(MANIFEST_ARCHIVE_PATH) {
            // Manifest text is read as UTF-8 so later checks can reason about field values
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .context("read preset manifest")?;
            manifest_contents = Some(contents);
            continue;
        }

        let Some(relative_path) = archive_payload_relative(&archive_path)? else {
            // Only payload entries are turned into bundle files
            continue;
        };
        if !entry.header().entry_type().is_file() {
            return Err(anyhow!(
                "preset bundle contains a non-file payload entry: {}",
                archive_path.display()
            ));
        }
        if files.contains_key(&relative_path) {
            // Duplicate payload paths would make import order-sensitive, so reject them
            return Err(anyhow!(
                "preset bundle contains duplicate payload entry: {}",
                format_relative_path(&relative_path)
            ));
        }

        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .with_context(|| format!("read bundle payload {}", archive_path.display()))?;
        let mode = entry.header().mode().unwrap_or(0o644);

        files.insert(
            relative_path.clone(),
            BundleFile {
                relative_path,
                contents,
                mode,
            },
        );
    }

    let manifest_contents =
        manifest_contents.ok_or_else(|| anyhow!("preset bundle is missing manifest.toml"))?;
    // Decode first, then fail hard on unsupported format versions
    let manifest = PresetManifest::decode(&manifest_contents).context("decode preset manifest")?;
    if manifest.format_version != PRESET_FORMAT_VERSION {
        return Err(anyhow!(
            "unsupported preset format version {}",
            manifest.format_version
        ));
    }

    let expected_paths = manifest
        .files
        .iter()
        .map(|file| (PathBuf::from(&file.path), file.size))
        .collect::<BTreeMap<_, _>>();
    let actual_paths = files
        .iter()
        .map(|(path, file)| (path.clone(), file.contents.len() as u64))
        .collect::<BTreeMap<_, _>>();
    if expected_paths != actual_paths {
        // Import trusts the manifest file list, so a mismatch means the bundle is corrupt
        let expected = expected_paths
            .keys()
            .map(|path| format_relative_path(path))
            .collect::<BTreeSet<_>>();
        let actual = actual_paths
            .keys()
            .map(|path| format_relative_path(path))
            .collect::<BTreeSet<_>>();
        return Err(anyhow!(
            "preset manifest file list does not match archive payload\nexpected: {:?}\nactual: {:?}",
            expected,
            actual
        ));
    }

    Ok(BundleArchive {
        manifest,
        files: files.into_values().collect(),
    })
}

fn append_bytes(
    builder: &mut Builder<GzEncoder<File>>,
    path: &Path,
    contents: &[u8],
    mode: u32,
) -> Result<()> {
    // Small in-memory writes are enough for the manifest
    let mut header = Header::new_gnu();
    header.set_mode(mode);
    header.set_size(contents.len() as u64);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(contents))
        .with_context(|| format!("append {} to preset archive", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{read_bundle, write_bundle};
    use crate::preset::filesystem::CollectedConfigFiles;
    use crate::preset::filesystem::PresetFileSource;
    use crate::preset::manifest::{PresetManifest, PresetManifestFile};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(name: &str) -> Self {
            // Unique temp roots keep tests isolated even when cargo runs them in parallel
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock moved backwards")
                .as_nanos();
            let serial = TEST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "unixnotis-preset-archive-{}-{}-{}",
                name, stamp, serial
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, relative_path: &str, contents: &str) -> PathBuf {
            // Test helpers build small fake config trees without touching the real config root
            let path = self.path.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent dirs");
            }
            fs::write(&path, contents).expect("write file");
            path
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn archive_round_trip_keeps_manifest_and_payload() {
        // Bundle reads should return the same file list that export wrote
        let root = TempDirGuard::new("roundtrip");
        let config_path = root.write("config.toml", "demo = true");
        let css_path = root.write("base.css", ".a { color: red; }");
        let bundle_path = root.path.join("demo.unixnotis");

        let collected = CollectedConfigFiles {
            files: vec![
                PresetFileSource {
                    relative_path: PathBuf::from("base.css"),
                    source_path: css_path,
                    size: 18,
                },
                PresetFileSource {
                    relative_path: PathBuf::from("config.toml"),
                    source_path: config_path,
                    size: 11,
                },
            ],
            skipped_symlinks: Vec::new(),
            skipped_non_regular: Vec::new(),
        };
        let manifest = PresetManifest::new(
            "demo".to_string(),
            "2026-04-11T12:00:00Z".to_string(),
            "0.1.0".to_string(),
            vec![
                PresetManifestFile {
                    path: "base.css".to_string(),
                    size: 18,
                },
                PresetManifestFile {
                    path: "config.toml".to_string(),
                    size: 11,
                },
            ],
        );

        write_bundle(&bundle_path, &manifest, &collected).expect("write bundle");
        let bundle = read_bundle(&bundle_path).expect("read bundle");

        assert_eq!(bundle.manifest.bundle_name, "demo");
        assert_eq!(bundle.files.len(), 2);
    }
}
