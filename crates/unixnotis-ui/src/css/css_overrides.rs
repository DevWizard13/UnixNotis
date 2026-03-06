//! Theme-driven CSS overrides used by the UI CSS manager.

use unixnotis_core::ThemeConfig;

pub(crate) fn build_base_overrides(theme: &ThemeConfig) -> String {
    // Clamp alpha values to avoid invalid CSS and keep overrides predictable.
    let surface_alpha = theme.surface_alpha.clamp(0.0, 1.0);
    let surface_strong_alpha = theme.surface_strong_alpha.clamp(0.0, 1.0);
    let shadow_soft = theme.shadow_soft_alpha.clamp(0.0, 1.0);
    let shadow_strong = theme.shadow_strong_alpha.clamp(0.0, 1.0);
    format!(
        r#"
@define-color unixnotis-surface alpha(@unixnotis-surface-base, {surface_alpha});
@define-color unixnotis-surface-strong alpha(@unixnotis-surface-strong-base, {surface_strong_alpha});
@define-color unixnotis-shadow-soft alpha(#000000, {shadow_soft});
@define-color unixnotis-shadow-strong alpha(#000000, {shadow_strong});
"#
    )
}

pub(crate) fn build_panel_overrides(theme: &ThemeConfig) -> String {
    let border_width = theme.border_width as f32;
    let card_radius = theme.card_radius as f32;
    let card_alpha = theme.card_alpha.clamp(0.0, 1.0);
    format!(
        r#"
.unixnotis-panel-card {{
  border-width: {border_width}px;
  border-style: solid;
  border-radius: {card_radius}px;
  background: alpha(@unixnotis-card-base, {card_alpha});
}}
"#
    )
}

pub(crate) fn build_widgets_overrides(theme: &ThemeConfig) -> String {
    let border_width = theme.border_width as f32;
    let card_radius = theme.card_radius as f32;
    let card_alpha = theme.card_alpha.clamp(0.0, 1.0);
    format!(
        r#"
.unixnotis-media-card {{
  border-width: {border_width}px;
  border-style: solid;
  border-radius: {card_radius}px;
  background: alpha(@unixnotis-card-base, {card_alpha});
}}
"#
    )
}

pub(crate) fn build_popup_overrides(theme: &ThemeConfig) -> String {
    let border_width = theme.border_width as f32;
    let card_radius = theme.card_radius as f32;
    format!(
        r#"
.unixnotis-popup-card {{
  border-width: {border_width}px;
  border-style: solid;
  border-radius: {card_radius}px;
}}
"#
    )
}

#[cfg(test)]
#[path = "css_overrides/tests.rs"]
mod tests;
