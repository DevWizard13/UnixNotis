//! Small CSS scanner helpers for css-check.

pub(super) fn strip_css_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_comment = false;
    while let Some(ch) = chars.next() {
        if in_comment {
            // Stay in comment mode until the closing marker is found
            if ch == '*' && matches!(chars.peek(), Some('/')) {
                chars.next();
                in_comment = false;
            }
            continue;
        }
        if ch == '/' && matches!(chars.peek(), Some('*')) {
            chars.next();
            in_comment = true;
            continue;
        }
        output.push(ch);
    }
    output
}

pub(super) fn next_css_block(bytes: &[u8], start: usize) -> Option<(String, String, usize)> {
    // This scanner only needs to find balanced blocks
    let mut selector_start = start;
    while selector_start < bytes.len() && bytes[selector_start].is_ascii_whitespace() {
        selector_start += 1;
    }

    let mut index = selector_start;
    let mut in_string: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(quote) = in_string {
            if byte == quote {
                in_string = None;
            }
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            in_string = Some(byte);
            index += 1;
            continue;
        }
        if byte == b'{' {
            let selector = String::from_utf8_lossy(&bytes[selector_start..index]).to_string();
            let mut depth = 1usize;
            index += 1;
            let block_start = index;
            while index < bytes.len() {
                let byte = bytes[index];
                if let Some(quote) = in_string {
                    if byte == quote {
                        in_string = None;
                    }
                    index += 1;
                    continue;
                }
                if byte == b'"' || byte == b'\'' {
                    in_string = Some(byte);
                    index += 1;
                    continue;
                }
                if byte == b'{' {
                    depth += 1;
                } else if byte == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        let block = String::from_utf8_lossy(&bytes[block_start..index]).to_string();
                        return Some((selector, block, index + 1));
                    }
                }
                index += 1;
            }
            break;
        }
        index += 1;
    }
    None
}

pub(super) fn normalize_selector(selector: &str) -> String {
    // Whitespace-only changes should not make selectors look different
    selector
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

pub(super) fn split_selectors(selector: &str) -> Vec<String> {
    // Split grouped selectors, but not commas inside nested parts
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0u32;
    let mut bracket_depth = 0u32;
    let mut in_string: Option<char> = None;
    let mut chars = selector.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(quote) = in_string {
            if ch == '\\' {
                current.push(ch);
                if let Some(next_char) = chars.next() {
                    current.push(next_char);
                }
                continue;
            }
            if ch == quote {
                in_string = None;
            }
            current.push(ch);
            continue;
        }

        match ch {
            '"' | '\'' => {
                in_string = Some(ch);
                current.push(ch);
            }
            '(' => {
                paren_depth = paren_depth.saturating_add(1);
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            '[' => {
                bracket_depth = bracket_depth.saturating_add(1);
                current.push(ch);
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if paren_depth == 0 && bracket_depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        parts.push(trimmed.to_string());
    }

    parts
}

pub(super) fn should_recurse_at_rule(selector: &str) -> bool {
    // Only recurse into at-rules that can hold nested selector blocks
    let name = selector
        .trim_start_matches('@')
        .split_whitespace()
        .next()
        .unwrap_or("");
    matches!(
        name,
        "media" | "supports" | "layer" | "container" | "document"
    )
}
