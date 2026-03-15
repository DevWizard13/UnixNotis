//! Geometry model and width-pressure math for css-check

use unixnotis_core::Config;

use super::stock::{stock_config, stock_geometry_model};

const WIDTH_WARNING_TOLERANCE_PX: i32 = 8;

const TOGGLE_GRID_COLUMNS: usize = 4;
const TOGGLE_GRID_SPACING_PX: i32 = 8;
const TOGGLE_FALLBACK_CONTENT_WIDTH_PX: i32 = 80;

const STAT_GRID_COLUMNS: usize = 2;
const STAT_GRID_SPACING_PX: i32 = 8;
const STAT_FALLBACK_CONTENT_WIDTH_PX: i32 = 96;

const CARD_GRID_COLUMNS: usize = 2;
const CARD_GRID_SPACING_PX: i32 = 8;
const CARD_FALLBACK_CONTENT_WIDTH_PX: i32 = 104;

const MEDIA_TEXT_WIDTH_FLOOR_PX: i32 = 140;
const MEDIA_NON_TEXT_BUDGET_PX: i32 = 240;
const MEDIA_ROW_SPACING_PX: i32 = 6;
const MEDIA_CARD_CONTENT_SPACING_PX: i32 = 10;
const MEDIA_CONTROL_BUTTON_SPACING_PX: i32 = 6;
const MEDIA_NAV_FALLBACK_WIDTH_PX: i32 = 22;
const MEDIA_ART_FALLBACK_WIDTH_PX: i32 = 50;
const MEDIA_ART_FRAME_FALLBACK_WIDTH_PX: i32 = 54;
const MEDIA_BUTTON_FALLBACK_WIDTH_PX: i32 = 28;

#[derive(Clone, Copy, Default)]
pub(super) struct HorizontalEdges {
    pub(super) left: f32,
    pub(super) right: f32,
}

impl HorizontalEdges {
    pub(super) fn total_px(self) -> i32 {
        // Rounding once keeps tiny float noise from leaking into warning math
        (self.left + self.right).round() as i32
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct HorizontalBoxMetrics {
    width: Option<f32>,
    min_width: Option<f32>,
    margin: HorizontalEdges,
    padding: HorizontalEdges,
    border: HorizontalEdges,
}

impl HorizontalBoxMetrics {
    pub(super) fn apply_property(&mut self, name: &str, value: &str) {
        // Only horizontal size inputs matter for this lint pass
        match name {
            "width" => self.width = super::parse::parse_single_length(value),
            "min-width" => self.min_width = super::parse::parse_single_length(value),
            "margin" => {
                // Shorthand values can set both left and right in one pass
                if let Some(edges) = super::parse::parse_box_edges(value) {
                    self.margin = edges;
                }
            }
            "margin-left" => super::parse::set_edge(&mut self.margin.left, value),
            "margin-right" => super::parse::set_edge(&mut self.margin.right, value),
            "padding" => {
                // Padding still eats panel width even when child content stays the same
                if let Some(edges) = super::parse::parse_box_edges(value) {
                    self.padding = edges;
                }
            }
            "padding-left" => super::parse::set_edge(&mut self.padding.left, value),
            "padding-right" => super::parse::set_edge(&mut self.padding.right, value),
            "border" | "border-width" => {
                if let Some(edges) = super::parse::parse_box_edges(value) {
                    self.border = edges;
                }
            }
            "border-left" | "border-left-width" => {
                super::parse::set_edge(&mut self.border.left, value)
            }
            "border-right" | "border-right-width" => {
                super::parse::set_edge(&mut self.border.right, value)
            }
            _ => {}
        }
    }

    pub(super) fn outer_width_px(self, fallback_px: i32) -> i32 {
        self.content_width_px(fallback_px)
            + self.margin.total_px()
            + self.padding.total_px()
            + self.border.total_px()
    }

    pub(super) fn outer_insets_px(self) -> i32 {
        // Outer insets affect the parent width budget directly
        self.margin.total_px() + self.padding.total_px() + self.border.total_px()
    }

    fn content_width_px(self, fallback_px: i32) -> i32 {
        // GTK can honor either width or min-width, so keep the larger one
        match (self.width, self.min_width) {
            (Some(width), Some(min_width)) => width.max(min_width).round() as i32,
            (Some(width), None) => width.round() as i32,
            (None, Some(min_width)) => min_width.round() as i32,
            (None, None) => fallback_px,
        }
    }

    fn inner_insets_px(self) -> i32 {
        // Panel padding and borders shrink the width left for child widgets
        self.padding.total_px() + self.border.total_px()
    }
}

#[derive(Default)]
pub(super) struct GeometryModel {
    panel: HorizontalBoxMetrics,
    toggle_section: HorizontalBoxMetrics,
    toggle_grid: HorizontalBoxMetrics,
    toggle_item: HorizontalBoxMetrics,
    stat_section: HorizontalBoxMetrics,
    stat_grid: HorizontalBoxMetrics,
    stat_item: HorizontalBoxMetrics,
    card_section: HorizontalBoxMetrics,
    card_grid: HorizontalBoxMetrics,
    card_item: HorizontalBoxMetrics,
    media_container: HorizontalBoxMetrics,
    media_stack: HorizontalBoxMetrics,
    media_row: HorizontalBoxMetrics,
    media_nav: HorizontalBoxMetrics,
    media_card: HorizontalBoxMetrics,
    media_art: HorizontalBoxMetrics,
    media_art_frame: HorizontalBoxMetrics,
    media_controls: HorizontalBoxMetrics,
    media_button: HorizontalBoxMetrics,
}

impl GeometryModel {
    pub(super) fn finalize_warnings(&self, config: &Config) -> Vec<String> {
        let mut warnings = Vec::new();

        // Each section is checked separately so the warning stays focused
        if let Some(warning) = self.toggle_grid_warning(config) {
            warnings.push(warning);
        }
        if let Some(warning) = self.stat_grid_warning(config) {
            warnings.push(warning);
        }
        if let Some(warning) = self.card_grid_warning(config) {
            warnings.push(warning);
        }
        if let Some(warning) = self.media_width_warning(config) {
            warnings.push(warning);
        }
        if let Some(warning) = self.media_art_target_warning() {
            warnings.push(warning);
        }

        warnings
    }

    pub(super) fn target_mut(&mut self, class_name: &str) -> Option<&mut HorizontalBoxMetrics> {
        // Only selectors that map to real width-owning widgets are tracked here
        match class_name {
            ".unixnotis-panel" => Some(&mut self.panel),
            ".unixnotis-toggle-section" => Some(&mut self.toggle_section),
            ".unixnotis-toggle-grid" => Some(&mut self.toggle_grid),
            ".unixnotis-toggle" => Some(&mut self.toggle_item),
            ".unixnotis-stat-section" => Some(&mut self.stat_section),
            ".unixnotis-stat-grid" => Some(&mut self.stat_grid),
            ".unixnotis-stat-card" => Some(&mut self.stat_item),
            ".unixnotis-card-section" => Some(&mut self.card_section),
            ".unixnotis-card-grid" => Some(&mut self.card_grid),
            ".unixnotis-info-card" => Some(&mut self.card_item),
            ".unixnotis-media-container" => Some(&mut self.media_container),
            ".unixnotis-media-stack" => Some(&mut self.media_stack),
            ".unixnotis-media-row" => Some(&mut self.media_row),
            ".unixnotis-media-nav" => Some(&mut self.media_nav),
            ".unixnotis-media-card" => Some(&mut self.media_card),
            ".unixnotis-media-art" => Some(&mut self.media_art),
            ".unixnotis-media-art-frame" => Some(&mut self.media_art_frame),
            ".unixnotis-media-controls" => Some(&mut self.media_controls),
            ".unixnotis-media-button" => Some(&mut self.media_button),
            _ => None,
        }
    }

    fn toggle_grid_warning(&self, config: &Config) -> Option<String> {
        let required_panel_width_px = self.toggle_required_panel_width_px(config)?;
        width_warning(
            "toggle grid",
            required_panel_width_px,
            config.panel.width,
            "GTK natural width can still widen the panel when fixed columns ask for more room",
        )
    }

    fn stat_grid_warning(&self, config: &Config) -> Option<String> {
        let required_panel_width_px = self.stat_required_panel_width_px(config)?;
        width_warning(
            "stat grid",
            required_panel_width_px,
            config.panel.width,
            "GTK natural width can still widen the panel when two-column cards ask for more room",
        )
    }

    fn card_grid_warning(&self, config: &Config) -> Option<String> {
        let required_panel_width_px = self.card_required_panel_width_px(config)?;
        width_warning(
            "card grid",
            required_panel_width_px,
            config.panel.width,
            "GTK natural width can still widen the panel when two-column cards ask for more room",
        )
    }

    fn media_width_warning(&self, config: &Config) -> Option<String> {
        if !config.media.enabled {
            // No media widget means no media width pressure
            return None;
        }
        let required_panel_width_px = self.media_required_panel_width_px(config);
        width_warning(
            "media row",
            required_panel_width_px,
            config.panel.width,
            "GTK natural width can still widen the panel when the media widget asks for more room",
        )
    }

    fn media_art_target_warning(&self) -> Option<String> {
        let art_width_px = self.media_art.outer_width_px(MEDIA_ART_FALLBACK_WIDTH_PX);
        let frame_width_px = self
            .media_art_frame
            .outer_width_px(MEDIA_ART_FRAME_FALLBACK_WIDTH_PX);
        if art_width_px <= frame_width_px + WIDTH_WARNING_TOLERANCE_PX {
            return None;
        }

        // The frame is what sets the outer slot width inside the card
        Some(format!(
            ".unixnotis-media-art now measures about {art_width_px}px while .unixnotis-media-art-frame is about {frame_width_px}px; the frame owns the outer card width, so picture-only sizing may not change the row the way the selector suggests"
        ))
    }

    fn toggle_required_panel_width_px(&self, config: &Config) -> Option<i32> {
        // Only enabled widgets can claim columns in the live grid
        let columns = config
            .widgets
            .toggles
            .iter()
            .filter(|widget| widget.enabled)
            .count()
            .min(TOGGLE_GRID_COLUMNS);
        if columns == 0 {
            return None;
        }

        // Panel padding, section padding, grid padding, item width, and spacing all add pressure
        let pressure = self.fixed_grid_pressure_px(
            columns,
            TOGGLE_GRID_SPACING_PX,
            self.toggle_section,
            self.toggle_grid,
            self.toggle_item
                .outer_width_px(TOGGLE_FALLBACK_CONTENT_WIDTH_PX),
        );

        // Compare against the shipped theme so stock CSS stays quiet
        let stock = stock_geometry_model();
        let stock_config = stock_config();
        let stock_columns = stock_config
            .widgets
            .toggles
            .iter()
            .filter(|widget| widget.enabled)
            .count()
            .min(TOGGLE_GRID_COLUMNS);
        // Compare against stock so shipped defaults stay quiet and only custom widening shows up
        let stock_pressure = stock.fixed_grid_pressure_px(
            stock_columns,
            TOGGLE_GRID_SPACING_PX,
            stock.toggle_section,
            stock.toggle_grid,
            stock
                .toggle_item
                .outer_width_px(TOGGLE_FALLBACK_CONTENT_WIDTH_PX),
        );

        Some(stock_config.panel.width + (pressure - stock_pressure))
    }

    fn stat_required_panel_width_px(&self, config: &Config) -> Option<i32> {
        // Stats render as a fixed two-column grid, so enabled items decide the live column count
        let columns = config
            .widgets
            .stats
            .iter()
            .filter(|widget| widget.enabled)
            .count()
            .min(STAT_GRID_COLUMNS);
        if columns == 0 {
            return None;
        }

        let pressure = self.fixed_grid_pressure_px(
            columns,
            STAT_GRID_SPACING_PX,
            self.stat_section,
            self.stat_grid,
            self.stat_item
                .outer_width_px(STAT_FALLBACK_CONTENT_WIDTH_PX),
        );

        let stock = stock_geometry_model();
        let stock_config = stock_config();
        let stock_columns = stock_config
            .widgets
            .stats
            .iter()
            .filter(|widget| widget.enabled)
            .count()
            .min(STAT_GRID_COLUMNS);
        // Stock subtraction keeps the warning centered on the user's CSS delta
        let stock_pressure = stock.fixed_grid_pressure_px(
            stock_columns,
            STAT_GRID_SPACING_PX,
            stock.stat_section,
            stock.stat_grid,
            stock
                .stat_item
                .outer_width_px(STAT_FALLBACK_CONTENT_WIDTH_PX),
        );

        Some(stock_config.panel.width + (pressure - stock_pressure))
    }

    fn card_required_panel_width_px(&self, config: &Config) -> Option<i32> {
        // Cards use the same fixed-column pattern as stats, with wider default content
        let columns = config
            .widgets
            .cards
            .iter()
            .filter(|widget| widget.enabled)
            .count()
            .min(CARD_GRID_COLUMNS);
        if columns == 0 {
            return None;
        }

        let pressure = self.fixed_grid_pressure_px(
            columns,
            CARD_GRID_SPACING_PX,
            self.card_section,
            self.card_grid,
            self.card_item
                .outer_width_px(CARD_FALLBACK_CONTENT_WIDTH_PX),
        );

        let stock = stock_geometry_model();
        let stock_config = stock_config();
        let stock_columns = stock_config
            .widgets
            .cards
            .iter()
            .filter(|widget| widget.enabled)
            .count()
            .min(CARD_GRID_COLUMNS);
        // Reuse the stock baseline so default cards do not trip the lint
        let stock_pressure = stock.fixed_grid_pressure_px(
            stock_columns,
            CARD_GRID_SPACING_PX,
            stock.card_section,
            stock.card_grid,
            stock
                .card_item
                .outer_width_px(CARD_FALLBACK_CONTENT_WIDTH_PX),
        );

        Some(stock_config.panel.width + (pressure - stock_pressure))
    }

    fn media_required_panel_width_px(&self, config: &Config) -> i32 {
        let pressure = self.media_pressure_px(config.panel.width);
        let stock = stock_geometry_model();
        let stock_config = stock_config();
        let stock_pressure = stock.media_pressure_px(stock_config.panel.width);

        // The delta from stock is easier to reason about than an absolute guess
        stock_config.panel.width + (pressure - stock_pressure)
    }

    fn fixed_grid_pressure_px(
        &self,
        columns: usize,
        spacing_px: i32,
        section: HorizontalBoxMetrics,
        grid: HorizontalBoxMetrics,
        item_outer_width_px: i32,
    ) -> i32 {
        // The panel loses width to its own chrome first
        self.panel.inner_insets_px()
            // Section and grid wrappers also eat width before items are laid out
            + section.outer_insets_px()
            + grid.outer_insets_px()
            // Each column brings its own outer box width
            + (columns as i32 * item_outer_width_px)
            // Spacing only exists between neighbors, never after the last item
            + ((columns.saturating_sub(1)) as i32 * spacing_px)
    }

    fn media_pressure_px(&self, panel_width_px: i32) -> i32 {
        // The live media widget already reserves a fixed non-text budget
        let text_width_px = panel_width_px
            .saturating_sub(MEDIA_NON_TEXT_BUDGET_PX)
            .max(MEDIA_TEXT_WIDTH_FLOOR_PX);

        // Buttons and artwork are the parts most likely to widen the row
        let controls_width_px = self.media_controls.outer_insets_px()
            + (self
                .media_button
                .outer_width_px(MEDIA_BUTTON_FALLBACK_WIDTH_PX)
                * 3)
            + (MEDIA_CONTROL_BUTTON_SPACING_PX * 2);
        // Card width is frame + gap + text + gap + controls
        let card_inner_width_px = self
            .media_art_frame
            .outer_width_px(MEDIA_ART_FRAME_FALLBACK_WIDTH_PX)
            + MEDIA_CARD_CONTENT_SPACING_PX
            + text_width_px
            + MEDIA_CARD_CONTENT_SPACING_PX
            + controls_width_px;
        let card_outer_width_px = self.media_card.outer_width_px(card_inner_width_px);

        // The full row also carries panel, container, stack, row, and nav button chrome
        self.panel.inner_insets_px()
            + self.media_container.outer_insets_px()
            + self.media_stack.outer_insets_px()
            + self.media_row.outer_insets_px()
            + card_outer_width_px
            + (self.media_nav.outer_width_px(MEDIA_NAV_FALLBACK_WIDTH_PX) * 2)
            + (MEDIA_ROW_SPACING_PX * 2)
    }
}

fn width_warning(
    label: &str,
    required_panel_width_px: i32,
    panel_width_px: i32,
    natural_width_note: &str,
) -> Option<String> {
    // Small rounding drift should not become a warning
    if required_panel_width_px <= panel_width_px + WIDTH_WARNING_TOLERANCE_PX {
        return None;
    }

    Some(format!(
        "{label} looks like it needs about {required_panel_width_px}px of panel width, but [panel].width={panel_width_px}; {natural_width_note}"
    ))
}
