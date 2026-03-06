use super::main_css_check_lint::lint_css_contents;
use super::main_css_check_parse::split_selectors;

#[test]
fn split_selectors_handles_is_commas() {
    // Commas inside :is() stay inside the selector
    let selector = ".a:is(.b, .c), .d";
    let parts = split_selectors(selector);
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], ".a:is(.b, .c)");
    assert_eq!(parts[1], ".d");
}

#[test]
fn lint_css_contents_scans_media_blocks() {
    // Nested media rules still report duplicate selectors
    let css = "@media (min-width: 1px) { .a { color: red; } .a { color: blue; } }";
    let warnings = lint_css_contents(css);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("duplicate selector '.a'"));
    assert!(warnings[0].contains("within @media (min-width: 1px)"));
}

#[test]
fn lint_css_contents_scans_layer_blocks() {
    // Layer blocks use the same duplicate selector rule
    let css = "@layer theme { .a { color: red; } .a { color: blue; } }";
    let warnings = lint_css_contents(css);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("duplicate selector '.a'"));
    assert!(warnings[0].contains("within @layer theme"));
}
