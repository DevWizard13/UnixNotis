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
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder, Header};

use super::config_root::CollectedConfigFiles;
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

    // Writing into a temp file first keeps partial bundles and symlink-follow writes off the target path
    let temp_path = temp_bundle_path(bundle_path);
    let output = File::create(&temp_path)
        .with_context(|| format!("create temp preset bundle {}", temp_path.display()))?;
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
        let mode = sanitize_payload_mode(file.mode, &file.relative_path)?;

        if let Some(contents) = &file.contents_override {
            // Overridden files stay in memory so export can patch config.toml in the bundle only
            append_bytes(
                &mut builder,
                &archive_payload_path(&file.relative_path),
                contents,
                mode,
            )?;
            continue;
        }

        // Files are streamed from disk so export memory stays bounded by one file at a time
        let mut source_file = File::open(&file.source_path)
            .with_context(|| format!("open {} for preset archive", file.source_path.display()))?;
        append_reader(
            &mut builder,
            &archive_payload_path(&file.relative_path),
            &mut source_file,
            file.size,
            mode,
        )?;
    }

    // Finish the tar writer first, then flush the gzip stream to disk
    builder.finish().context("finish preset archive")?;
    let encoder = builder
        .into_inner()
        .context("flush preset archive writer")?;
    let output = encoder.finish().context("finalize preset bundle")?;
    output
        .sync_all()
        .with_context(|| format!("flush temp preset bundle {}", temp_path.display()))?;

    if let Err(err) = fs::rename(&temp_path, bundle_path)
        .with_context(|| format!("replace preset bundle {}", bundle_path.display()))
    {
        // Temp bundle cleanup keeps failed exports from leaving large junk files behind
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
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
        let mode =
            sanitize_payload_mode(entry.header().mode().unwrap_or(0o644), &relative_path)?;

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

fn append_reader<R: Read>(
    builder: &mut Builder<GzEncoder<File>>,
    path: &Path,
    reader: &mut R,
    size: u64,
    mode: u32,
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_mode(mode);
    header.set_size(size);
    header.set_cksum();
    builder
        .append_data(&mut header, path, reader)
        .with_context(|| format!("append {} to preset archive", path.display()))?;
    Ok(())
}

fn sanitize_payload_mode(mode: u32, relative_path: &Path) -> Result<u32> {
    // Keep only permission bits and reject setuid/setgid/sticky flags from preset payloads
    let permission_mode = mode & 0o7777;
    if permission_mode & 0o7000 != 0 {
        return Err(anyhow!(
            "preset payload contains unsupported special permission bits: {}",
            relative_path.display()
        ));
    }
    Ok(permission_mode & 0o777)
}

fn temp_bundle_path(bundle_path: &Path) -> PathBuf {
    let parent = bundle_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = bundle_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("preset.unixnotis");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock moved backwards")
        .as_nanos();
    parent.join(format!(".{file_name}.{stamp}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::{read_bundle, write_bundle};
    use crate::preset::config_root::CollectedConfigFiles;
    use crate::preset::config_root::PresetFileSource;
    use crate::preset::manifest::{PresetManifest, PresetManifestFile};
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tar::{Builder, Header};

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
                    mode: 0o644,
                    contents_override: None,
                },
                PresetFileSource {
                    relative_path: PathBuf::from("config.toml"),
                    source_path: config_path,
                    size: 11,
                    mode: 0o644,
                    contents_override: None,
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

    #[test]
    fn archive_round_trip_uses_overridden_file_bytes() {
        let root = TempDirGuard::new("override");
        let config_path = root.write("config.toml", "demo = true");
        let bundle_path = root.path.join("demo.unixnotis");

        let collected = CollectedConfigFiles {
            files: vec![PresetFileSource {
                relative_path: PathBuf::from("config.toml"),
                source_path: config_path,
                size: 12,
                mode: 0o644,
                contents_override: Some(b"demo = false\n".to_vec()),
            }],
            skipped_symlinks: Vec::new(),
            skipped_non_regular: Vec::new(),
        };
        let manifest = PresetManifest::new(
            "demo".to_string(),
            "2026-04-11T12:00:00Z".to_string(),
            "0.1.0".to_string(),
            vec![PresetManifestFile {
                path: "config.toml".to_string(),
                size: 13,
            }],
        );

        write_bundle(&bundle_path, &manifest, &collected).expect("write bundle");
        let bundle = read_bundle(&bundle_path).expect("read bundle");

        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].contents, b"demo = false\n");
    }

    #[test]
    fn read_bundle_rejects_special_permission_bits() {
        let root = TempDirGuard::new("special-mode");
        let bundle_path = root.path.join("demo.unixnotis");
        let output = fs::File::create(&bundle_path).expect("create bundle");
        let encoder = GzEncoder::new(output, Compression::default());
        let mut builder = Builder::new(encoder);

        let manifest = PresetManifest::new(
            "demo".to_string(),
            "2026-04-11T12:00:00Z".to_string(),
            "0.1.0".to_string(),
            vec![PresetManifestFile {
                path: "config.toml".to_string(),
                size: 12,
            }],
        );
        let manifest_bytes = manifest.encode().expect("encode manifest").into_bytes();
        let mut manifest_header = Header::new_gnu();
        manifest_header.set_mode(0o644);
        manifest_header.set_size(manifest_bytes.len() as u64);
        manifest_header.set_cksum();
        builder
            .append_data(
                &mut manifest_header,
                Path::new("manifest.toml"),
                Cursor::new(manifest_bytes),
            )
            .expect("append manifest");

        let payload = b"demo = true\n";
        let mut payload_header = Header::new_gnu();
        payload_header.set_mode(0o4755);
        payload_header.set_size(payload.len() as u64);
        payload_header.set_cksum();
        builder
            .append_data(
                &mut payload_header,
                Path::new("payload/config.toml"),
                Cursor::new(payload),
            )
            .expect("append payload");
        builder.finish().expect("finish archive");
        let encoder = builder.into_inner().expect("take encoder");
        let file = encoder.finish().expect("finish gzip");
        file.sync_all().expect("sync bundle");

        let error = read_bundle(&bundle_path).expect_err("reject special mode bits");
        assert!(error.to_string().contains("special permission bits"));
    }
}
