//! Images in the message view (design, "Files and images"): decoded with
//! `image`, scaled down to the terminal, kept for the last few items, and
//! drawn with ratatui-image in the terminal's graphics protocol (sixel,
//! kitty, iTerm2, else half blocks).

use mxt_core::timeline::ItemRef;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{FilterType, Resize, StatefulImage};

/// How many decoded images are kept.
const KEPT: usize = 4;

pub(crate) struct Images {
    picker: Picker,
    kept: Recent<ItemRef, StatefulProtocol>,
}

impl Images {
    /// Asks the terminal for its graphics protocol and font size, and uses
    /// half blocks if that fails. The terminal must be in raw mode, and
    /// nothing else may read its input yet: the answers arrive there.
    pub(crate) fn query() -> Images {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        Images {
            picker,
            kept: Recent::new(KEPT),
        }
    }

    /// Decodes `bytes`, the image of `item`, unless it is kept already, and
    /// keeps it scaled down to fit `screen` (the terminal's size in cells).
    pub(crate) fn insert(
        &mut self,
        item: ItemRef,
        bytes: &[u8],
        screen: Size,
    ) -> image::ImageResult<()> {
        if self.kept.get_mut(&item).is_some() {
            return Ok(());
        }
        let image = image::load_from_memory(bytes)?;
        let font = self.picker.font_size();
        let width = u32::from(screen.width) * u32::from(font.width);
        let height = u32::from(screen.height) * u32::from(font.height);
        let image = if image.width() > width || image.height() > height {
            image.thumbnail(width, height)
        } else {
            image
        };
        self.kept
            .insert(item, self.picker.new_resize_protocol(image));
        Ok(())
    }

    /// Draws the image of `item` in the top left of `area`, scaled down to
    /// fit, once it is kept.
    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, item: &ItemRef) {
        if let Some(image) = self.kept.get_mut(item) {
            let widget = StatefulImage::new().resize(Resize::Fit(Some(FilterType::Triangle)));
            frame.render_stateful_widget(widget, area, image);
        }
    }
}

/// The values of the last `capacity` keys inserted or looked up.
struct Recent<K, V> {
    capacity: usize,
    /// The least recently used first.
    entries: Vec<(K, V)>,
}

impl<K: PartialEq, V> Recent<K, V> {
    fn new(capacity: usize) -> Recent<K, V> {
        Recent {
            capacity,
            entries: Vec::with_capacity(capacity),
        }
    }

    /// The value of `key`, which becomes the most recently used.
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(index);
        self.entries.push(entry);
        self.entries.last_mut().map(|(_, value)| value)
    }

    /// Keeps `value` as the most recently used, in place of the value of the
    /// same key or else of the least recently used key when full.
    fn insert(&mut self, key: K, value: V) {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(index);
        } else if self.entries.len() == self.capacity {
            self.entries.remove(0);
        }
        self.entries.push((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(recent: &Recent<u32, &str>) -> Vec<u32> {
        recent.entries.iter().map(|(key, _)| *key).collect()
    }

    #[test]
    fn the_least_recently_used_image_is_dropped_when_full() {
        let mut recent = Recent::new(2);
        recent.insert(1, "a");
        recent.insert(2, "b");
        // Looking 1 up makes 2 the least recently used.
        assert_eq!(recent.get_mut(&1), Some(&mut "a"));
        recent.insert(3, "c");
        assert_eq!(keys(&recent), [1, 3]);
        assert_eq!(recent.get_mut(&2), None);
    }

    #[test]
    fn inserting_a_kept_key_replaces_its_value_without_dropping_another() {
        let mut recent = Recent::new(2);
        recent.insert(1, "a");
        recent.insert(2, "b");
        recent.insert(1, "A");
        assert_eq!(keys(&recent), [2, 1]);
        assert_eq!(recent.get_mut(&1), Some(&mut "A"));
    }
}
