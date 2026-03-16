use unixnotis_core::MediaLayout;

const MEDIA_TEXT_WIDTH_FLOOR_PX: i32 = 140;
// Carousel keeps the widest fixed chrome because nav lives outside the card
const CAROUSEL_TEXT_RESERVE_PX: i32 = 240;
// Inline pulls nav into the card and frees some title space
const INLINE_TEXT_RESERVE_PX: i32 = 196;
// Stacked moves controls under the header so width pressure is much lower
const STACKED_TEXT_RESERVE_PX: i32 = 116;
// Showcase spends width on the side rail instead of outer nav buttons
const SHOWCASE_TEXT_RESERVE_PX: i32 = 224;

pub(super) fn stack_layout_class(layout: MediaLayout) -> &'static str {
    // Stable classes let media.css style each shell without guessing structure
    match layout {
        MediaLayout::Carousel => "unixnotis-media-stack-carousel",
        MediaLayout::Inline => "unixnotis-media-stack-inline",
        MediaLayout::Stacked => "unixnotis-media-stack-stacked",
        MediaLayout::Showcase => "unixnotis-media-stack-showcase",
    }
}

pub(super) fn row_layout_class(layout: MediaLayout) -> &'static str {
    // Row classes mirror the shell preset so width tweaks can stay layout specific
    match layout {
        MediaLayout::Carousel => "unixnotis-media-row-carousel",
        MediaLayout::Inline => "unixnotis-media-row-inline",
        MediaLayout::Stacked => "unixnotis-media-row-stacked",
        MediaLayout::Showcase => "unixnotis-media-row-showcase",
    }
}

pub(super) fn card_layout_class(layout: MediaLayout) -> &'static str {
    // Card classes are the main theme hook users touch when ricing the player
    match layout {
        MediaLayout::Carousel => "unixnotis-media-card-carousel",
        MediaLayout::Inline => "unixnotis-media-card-inline",
        MediaLayout::Stacked => "unixnotis-media-card-stacked",
        MediaLayout::Showcase => "unixnotis-media-card-showcase",
    }
}

pub(super) fn marquee_width_for_layout(layout: MediaLayout, panel_width: i32) -> i32 {
    // Each preset spends panel width differently, so keep one reserve budget per layout
    let reserve_px = match layout {
        MediaLayout::Carousel => CAROUSEL_TEXT_RESERVE_PX,
        MediaLayout::Inline => INLINE_TEXT_RESERVE_PX,
        MediaLayout::Stacked => STACKED_TEXT_RESERVE_PX,
        MediaLayout::Showcase => SHOWCASE_TEXT_RESERVE_PX,
    };
    panel_width
        .saturating_sub(reserve_px)
        // Tiny panel widths still keep a minimum readable title lane
        .max(MEDIA_TEXT_WIDTH_FLOOR_PX)
}

pub(super) fn card_height_for_layout(layout: MediaLayout) -> i32 {
    match layout {
        // Carousel keeps the old single-row pill height
        MediaLayout::Carousel => 72,
        // Inline gets one extra row for the folded nav strip
        MediaLayout::Inline => 92,
        // Stacked keeps enough room for header plus control strip
        MediaLayout::Stacked => 112,
        // Showcase keeps one taller hero row with a side rail
        MediaLayout::Showcase => 96,
    }
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
