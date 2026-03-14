use crate::media::MediaInfo;

#[derive(Default)]
pub(super) struct MediaSelection {
    pub(super) players: Vec<MediaInfo>,
    current_index: usize,
}

impl MediaSelection {
    pub(super) fn set_players(&mut self, players: Vec<MediaInfo>) {
        // Hold onto the current bus so refreshes do not kick the user back to slot one
        let current_bus = self.current_bus();
        self.players = players;
        if self.players.is_empty() {
            self.current_index = 0;
            return;
        }
        if let Some(current_bus) = current_bus {
            if let Some(index) = self
                .players
                .iter()
                .position(|info| info.bus_name == current_bus)
            {
                // Keep the same visible player when the snapshot order changes
                self.current_index = index;
                return;
            }
        }
        // If the previous player is gone, start from the first visible entry again
        self.current_index = 0;
    }

    pub(super) fn current(&self) -> Option<&MediaInfo> {
        self.players.get(self.current_index)
    }

    pub(super) fn current_bus(&self) -> Option<String> {
        self.current().map(|info| info.bus_name.clone())
    }

    pub(super) fn next(&mut self) {
        if self.players.len() <= 1 {
            return;
        }
        // Wraparound keeps the carousel controls simple
        self.current_index = (self.current_index + 1) % self.players.len();
    }

    pub(super) fn prev(&mut self) {
        if self.players.len() <= 1 {
            return;
        }
        if self.current_index == 0 {
            // Reverse navigation wraps to the last card
            self.current_index = self.players.len() - 1;
        } else {
            self.current_index -= 1;
        }
    }

    pub(super) fn has_multiple(&self) -> bool {
        self.players.len() > 1
    }

    pub(super) fn position(&self) -> (usize, usize) {
        if self.players.is_empty() {
            return (0, 0);
        }
        // The UI shows one-based positions because that reads better in the pill
        (self.current_index + 1, self.players.len())
    }
}
