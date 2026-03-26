use unixnotis_core::MediaLayout;

use super::{card_height_for_layout, marquee_width_for_layout, media_content_width};

#[test]
fn media_layout_reserve_budgets_stay_ordered() {
    let panel_width = 420;
    let carousel = marquee_width_for_layout(MediaLayout::Carousel, panel_width);
    let inline = marquee_width_for_layout(MediaLayout::Inline, panel_width);
    let stacked = marquee_width_for_layout(MediaLayout::Stacked, panel_width);
    let showcase = marquee_width_for_layout(MediaLayout::Showcase, panel_width);

    // Carousel keeps the biggest non-text reserve because nav stays outside the card
    assert!(carousel < inline);
    // Showcase keeps a dedicated side rail, so it still leaves less room than inline
    assert!(showcase < inline);
    assert!(inline < stacked);
}

#[test]
fn media_layout_height_presets_match_expected_profiles() {
    assert_eq!(card_height_for_layout(MediaLayout::Carousel), 72);
    assert_eq!(card_height_for_layout(MediaLayout::Inline), 92);
    assert_eq!(card_height_for_layout(MediaLayout::Stacked), 112);
    assert_eq!(card_height_for_layout(MediaLayout::Showcase), 96);
}

#[test]
fn marquee_width_never_drops_below_floor() {
    for layout in [
        MediaLayout::Carousel,
        MediaLayout::Inline,
        MediaLayout::Stacked,
        MediaLayout::Showcase,
    ] {
        assert_eq!(marquee_width_for_layout(layout, 80), 140);
    }
}

#[test]
fn media_content_width_reserves_panel_surface_chrome() {
    assert_eq!(media_content_width(420), 384);
    assert_eq!(media_content_width(20), 1);
}

#[test]
fn marquee_width_uses_inner_panel_width_budget() {
    assert_eq!(marquee_width_for_layout(MediaLayout::Carousel, 420), 144);
    assert_eq!(marquee_width_for_layout(MediaLayout::Inline, 420), 188);
    assert_eq!(marquee_width_for_layout(MediaLayout::Stacked, 420), 268);
    assert_eq!(marquee_width_for_layout(MediaLayout::Showcase, 420), 160);
}
