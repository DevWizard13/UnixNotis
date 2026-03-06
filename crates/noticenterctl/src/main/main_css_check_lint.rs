//! Lint rules for css-check.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::main_css_check_files::format_display_path;
use super::main_css_check_parse::{
    next_css_block, normalize_selector, should_recurse_at_rule, split_selectors, strip_css_comments,
};

pub(super) fn lint_css_files(
    files: &[PathBuf],
    config_dir: &Path,
    display_root: &str,
) -> Result<usize> {
    let mut warnings = 0usize;
    for path in files {
        let display_path = format_display_path(config_dir, display_root, path);

        // GTK only reports parse errors, so lint reads the file text too
        let contents = fs::read_to_string(path)
            .with_context(|| format!("read css file {}", path.display()))?;
        let report = lint_css_contents(&contents);
        for warning in report {
            warnings += 1;
            eprintln!("css warning: {}: {}", display_path, warning);
        }
    }
    Ok(warnings)
}

pub(super) fn lint_css_contents(contents: &str) -> Vec<String> {
    let mut warnings = Vec::new();

    // Remove comments first so the scanner sees real blocks only
    let stripped = strip_css_comments(contents);

    // Repeated color names usually mean accidental override order
    let mut color_defs: HashMap<String, usize> = HashMap::new();
    for line in stripped.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("@define-color") {
            if let Some(name) = rest.split_whitespace().next() {
                let count = color_defs.entry(name.to_string()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    warnings.push(format!(
                        "duplicate @define-color '{}' (later definition overrides earlier)",
                        name
                    ));
                }
            }
        }
    }

    // Selector repeats are tracked across the whole file
    let mut selector_seen: HashMap<String, usize> = HashMap::new();
    lint_css_block(&stripped, None, &mut selector_seen, &mut warnings);
    warnings
}

fn lint_css_block(
    contents: &str,
    context: Option<String>,
    selector_seen: &mut HashMap<String, usize>,
    warnings: &mut Vec<String>,
) {
    let mut cursor = 0usize;
    let bytes = contents.as_bytes();
    while let Some((selector, block, next)) = next_css_block(bytes, cursor) {
        cursor = next;
        let selector = normalize_selector(&selector);
        if selector.is_empty() {
            continue;
        }

        if selector.starts_with('@') {
            if should_recurse_at_rule(&selector) {
                let nested_context = match context.as_ref() {
                    Some(parent) => format!("{parent} {selector}"),
                    None => selector.clone(),
                };
                // Keep at-rule context so warnings still point to the right scope
                lint_css_block(&block, Some(nested_context), selector_seen, warnings);
            }
            continue;
        }

        // Grouped selectors need to be tracked one by one
        for selector_part in split_selectors(&selector) {
            if selector_part.is_empty() {
                continue;
            }
            let key = match context.as_ref() {
                Some(prefix) => format!("{prefix}::{selector_part}"),
                None => selector_part.clone(),
            };
            let count = selector_seen.entry(key).or_insert(0);
            *count += 1;
            if *count > 1 {
                let context_note = context
                    .as_ref()
                    .map(|ctx| format!(" within {ctx}"))
                    .unwrap_or_default();
                warnings.push(format!(
                    "duplicate selector '{}'{} (later rules override earlier)",
                    selector_part, context_note
                ));
            }
        }

        warnings.extend(lint_css_properties(&selector, &block, context.as_deref()));
    }
}

fn lint_css_properties(selector: &str, block: &str, context: Option<&str>) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for chunk in block.split(';') {
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some((name, _)) = trimmed.split_once(':') else {
            continue;
        };
        let prop = name.trim();
        if prop.is_empty() {
            continue;
        }
        if !seen.insert(prop.to_string()) {
            let context_note = context
                .map(|ctx| format!(" within {ctx}"))
                .unwrap_or_default();
            warnings.push(format!(
                "duplicate property '{}' in selector '{}'{} (later value overrides earlier)",
                prop, selector, context_note
            ));
        }
    }
    warnings
}
