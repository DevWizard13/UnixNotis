//! Stock theme helpers for geometry lint

use std::collections::HashSet;
use std::sync::OnceLock;

use unixnotis_core::{
    Config, DEFAULT_BASE_CSS, DEFAULT_PANEL_CSS, DEFAULT_POPUP_CSS, DEFAULT_WIDGETS_CSS,
};

use super::model::GeometryModel;
use super::parse::collect_geometry_from_contents;

pub(super) fn known_unixnotis_classes() -> &'static HashSet<&'static str> {
    static CLASSES: OnceLock<HashSet<&'static str>> = OnceLock::new();
    CLASSES.get_or_init(|| {
        let mut classes = HashSet::new();
        // Scan the shipped theme once and reuse the set for every lint run
        for css in [
            DEFAULT_BASE_CSS,
            DEFAULT_PANEL_CSS,
            DEFAULT_POPUP_CSS,
            DEFAULT_WIDGETS_CSS,
        ] {
            collect_unixnotis_classes(css, &mut classes);
        }
        classes
    })
}

pub(super) fn stock_config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    // Default config is a stable baseline for false-positive control
    CONFIG.get_or_init(Config::default)
}

pub(super) fn stock_geometry_model() -> &'static GeometryModel {
    static MODEL: OnceLock<GeometryModel> = OnceLock::new();
    MODEL.get_or_init(|| {
        let mut model = GeometryModel::default();
        // Merge every shipped CSS file into one baseline geometry model
        for css in [
            DEFAULT_BASE_CSS,
            DEFAULT_PANEL_CSS,
            DEFAULT_POPUP_CSS,
            DEFAULT_WIDGETS_CSS,
        ] {
            // The shipped theme is the baseline used to keep false positives low
            let _ = collect_geometry_from_contents(css, &mut model);
        }
        model
    })
}

fn collect_unixnotis_classes(css: &'static str, classes: &mut HashSet<&'static str>) {
    let bytes = css.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'.' || !css[index + 1..].starts_with("unixnotis-") {
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < bytes.len() {
            let byte = bytes[index];
            if !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_') {
                break;
            }
            index += 1;
        }

        // Borrow slices from the static CSS string so no extra allocation is needed
        classes.insert(&css[start..index]);
    }
}
