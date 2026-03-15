//! CSS validation and lint helpers for UnixNotis themes.

#[path = "main_css_check_files.rs"]
mod main_css_check_files;
#[path = "main_css_check_geometry.rs"]
mod main_css_check_geometry;
#[path = "main_css_check_lint.rs"]
mod main_css_check_lint;
#[path = "main_css_check_parse.rs"]
mod main_css_check_parse;
#[path = "main_css_check_runtime.rs"]
mod main_css_check_runtime;
#[path = "main_css_check_theme.rs"]
mod main_css_check_theme;

use anyhow::{anyhow, Context, Result};
use gtk::prelude::*;
use gtk::CssProvider;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use unixnotis_core::Config;

use self::main_css_check_files::{display_config_root, format_display_path};
use self::main_css_check_geometry::lint_geometry_css_files;
use self::main_css_check_lint::lint_css_files;
use self::main_css_check_runtime::lint_runtime_config;
use self::main_css_check_theme::collect_css_check_inputs;

pub(crate) fn run_css_check() -> Result<()> {
    // GTK must be ready before CSS parsing is used
    gtk::init().context("initialize gtk")?;

    // CSS check always scans the live config tree
    let config_dir = Config::default_config_dir().context("resolve config directory")?;
    let display_root = display_config_root(&config_dir);
    if !config_dir.exists() {
        return Err(anyhow!("config directory not found: {}", display_root));
    }
    if !config_dir.is_dir() {
        return Err(anyhow!("config path is not a directory: {}", display_root));
    }

    // Follow the same active theme targets the live UI resolves from config.toml
    let css_inputs = collect_css_check_inputs(&config_dir, &display_root)?;
    for line in &css_inputs.info_lines {
        println!("{line}");
    }
    let css_files = css_inputs.files;
    if css_files.is_empty() {
        return Err(anyhow!("no active css files found for {}", display_root));
    }

    // Count parser failures and print each one as GTK reports it
    let error_count = Arc::new(AtomicUsize::new(0));
    let provider = CssProvider::new();
    let error_count_clone = Arc::clone(&error_count);
    let config_root = config_dir.clone();
    let display_root_clone = display_root.clone();
    provider.connect_parsing_error(move |_provider, section, error| {
        error_count_clone.fetch_add(1, Ordering::Relaxed);
        let location = section.start_location();
        // GTK reports the source section, so convert it into the same path style used elsewhere
        let file = section
            .file()
            .and_then(|file| file.path())
            .map(|path| format_display_path(&config_root, &display_root_clone, &path))
            .unwrap_or_else(|| "<data>".to_string());
        eprintln!(
            "css error: {}:{}:{}: {}",
            file,
            location.lines() + 1,
            location.line_chars() + 1,
            error.message()
        );
        // The raw GTK error is often short, so print one extra hint from the broken line when possible
        if let Some(line_text) = source_line_text(
            section.file().and_then(|file| file.path()).as_deref(),
            location.lines() + 1,
        ) {
            if let Some(hint) = parsing_error_hint(&line_text) {
                eprintln!(
                    "css hint: {}:{}:{}: {}",
                    file,
                    location.lines() + 1,
                    location.line_chars() + 1,
                    hint
                );
            }
        }
    });

    for path in &css_files {
        // Bad paths are reported before GTK tries to parse them
        if !path.exists() {
            error_count.fetch_add(1, Ordering::Relaxed);
            let display_path = format_display_path(&config_dir, &display_root, path);
            eprintln!("css error: {}: file not found", display_path);
            continue;
        }
        if !path.is_file() {
            error_count.fetch_add(1, Ordering::Relaxed);
            let display_path = format_display_path(&config_dir, &display_root, path);
            eprintln!("css error: {}: not a regular file", display_path);
            continue;
        }
        // Feed each file into the same provider so GTK parses them like the live app does
        provider.load_from_path(path);
    }

    // GTK parse errors fail the command
    let errors = error_count.load(Ordering::Relaxed);
    if errors > 0 {
        return Err(anyhow!(
            "css-check found {} error(s) under {}",
            errors,
            display_root
        ));
    }

    // Theme-path warnings come first because they explain which files were actually checked
    let mut warnings = 0usize;
    for warning in css_inputs.warnings {
        warnings += 1;
        eprintln!("css warning: {}: {}", warning.display_path, warning.message);
    }

    // Lint warnings are useful, but they do not block valid CSS
    warnings += lint_css_files(&css_files, &config_dir, &display_root)?;
    // Live config can still override how css feels at runtime, so report those clashes too
    warnings += lint_runtime_config(&config_dir, &display_root)?;
    // Geometry warnings look for child layouts that can outgrow the requested panel width
    warnings += lint_geometry_css_files(&css_files, &config_dir, &display_root)?;
    if warnings > 0 {
        println!(
            "css-check warnings: {} issue(s) under {}",
            warnings, display_root
        );
    }

    println!(
        "css-check ok: {} file(s) checked under {}",
        css_files.len(),
        display_root
    );
    Ok(())
}

fn source_line_text(path: Option<&Path>, line_number: usize) -> Option<String> {
    let path = path?;
    if line_number == 0 {
        return None;
    }
    // Read on demand so the happy path does not carry file contents around
    let contents = fs::read_to_string(path).ok()?;
    contents
        .lines()
        // GTK line numbers are one-based, while iterator indexing is zero-based
        .nth(line_number.saturating_sub(1))
        .map(str::to_string)
}

fn parsing_error_hint(line_text: &str) -> Option<&'static str> {
    // Trim once so hint checks are not thrown off by surrounding whitespace
    let trimmed = line_text.trim();
    if trimmed.contains('%') {
        // GTK size properties are much stricter than web CSS
        return Some(
            "percentage lengths often fail in GTK CSS size properties; prefer fixed pixel values",
        );
    }
    if trimmed.contains("calc(") {
        // calc() is a common web habit that GTK CSS does not handle here
        return Some(
            "GTK CSS does not support calc() in layout properties here; use a fixed value instead",
        );
    }
    if trimmed.contains("var(") {
        // GTK supports @define-color, not web CSS custom properties
        return Some("GTK CSS does not use var() custom properties; use @define-color for colors and fixed lengths for layout");
    }
    None
}

#[cfg(test)]
#[path = "main_css_check_tests.rs"]
mod tests;
