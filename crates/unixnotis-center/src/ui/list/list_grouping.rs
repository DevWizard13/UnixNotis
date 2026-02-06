//! Grouping key normalization and list consistency helpers.

use std::borrow::Cow;
use std::rc::Rc;

use super::NotificationList;

impl NotificationList {
    pub(super) fn intern_key(&mut self, key: &str) -> Rc<str> {
        let normalized = self.normalize_group_key(key);
        if let Some(value) = self.interned.get(normalized.as_ref()) {
            return value.clone();
        }
        // Normalize app names to avoid duplicate groups from case/whitespace variations.
        let value: Rc<str> = Rc::from(normalized.as_ref());
        self.interned.insert(value.clone());
        value
    }

    pub(super) fn normalize_group_key<'a>(&self, key: &'a str) -> Cow<'a, str> {
        // Trim outer whitespace to avoid duplicate stacks from padded app names.
        let trimmed = key.trim();
        if trimmed.is_empty() {
            return Cow::Borrowed("");
        }
        let mut normalized = String::new();
        // Track normalization to avoid allocations when the key is already clean.
        let mut changed = false;
        for ch in trimmed.chars() {
            if is_ignorable_group_char(ch) {
                // Strip invisible characters to keep visually identical names grouped.
                changed = true;
                continue;
            }
            if ch.is_ascii_uppercase() {
                // ASCII-only casing keeps stable group keys without locale-dependent transforms.
                normalized.push(ch.to_ascii_lowercase());
                changed = true;
            } else {
                normalized.push(ch);
            }
        }
        if normalized.is_empty() {
            return Cow::Borrowed("");
        }
        if changed {
            return Cow::Owned(normalized);
        }
        // Trim-only normalization keeps display text stable while grouping remains consistent.
        Cow::Borrowed(trimmed)
    }

    pub(super) fn expected_list_len(&self) -> usize {
        // Sum group block sizes to mirror the visible list length (headers + rows + ghosts).
        self.group_order
            .iter()
            .filter_map(|key| self.grouped_cache.get(key).map(|ids| (key, ids)))
            .filter_map(|(key, ids)| {
                let visible = self.visible_ids_for_group(ids);
                if visible.is_empty() {
                    return None;
                }
                Some(self.group_block_len(key, visible.as_ref()))
            })
            .sum()
    }

    pub(super) fn group_has_visible_entries(&self, ids: &[u32]) -> bool {
        if self.filter_query.is_none() {
            return !ids.is_empty();
        }
        ids.iter().any(|id| {
            self.entries
                .get(id)
                .map(|entry| self.entry_matches_filter(&entry.view))
                .unwrap_or(false)
        })
    }

    pub(super) fn visible_ids_for_group<'a>(&self, ids: &'a [u32]) -> Cow<'a, [u32]> {
        if self.filter_query.is_none() {
            return Cow::Borrowed(ids);
        }
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(entry) = self.entries.get(id) {
                if self.entry_matches_filter(&entry.view) {
                    out.push(*id);
                }
            }
        }
        Cow::Owned(out)
    }

    pub(super) fn normalize_filter_query(&self, query: &str) -> Option<String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_lowercase())
    }

    fn entry_matches_filter(&self, view: &unixnotis_core::NotificationView) -> bool {
        let Some(query) = self.filter_query.as_deref() else {
            return true;
        };
        contains_casefold(&view.app_name, query)
            || contains_casefold(&view.summary, query)
            || contains_casefold(&view.body, query)
    }
}

fn is_ignorable_group_char(ch: char) -> bool {
    // Strip control/zero-width characters to keep grouping stable for visually identical names.
    ch.is_control()
        || matches!(
            ch,
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
        )
}

fn contains_casefold(haystack: &str, needle_lower: &str) -> bool {
    // Full Unicode lowercasing keeps filter matching behavior predictable
    // for non-ASCII notification text.
    haystack.to_lowercase().contains(needle_lower)
}
