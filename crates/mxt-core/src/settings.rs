//! The settings the core needs, converted from the configuration by `mxt`.

use crate::rooms::RoomSort;

/// What the message view shows for images (`ui.image_preview`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ImagePreview {
    /// The image, in the graphics protocol chosen for the terminal.
    #[default]
    Auto,
    /// The image, in this protocol whatever the terminal is.
    Forced(ImageProtocol),
    /// Only the file name and size.
    Off,
}

/// A way to draw images in the terminal, for `ui.image_preview`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProtocol {
    Iterm2,
    Sixel,
    Kitty,
    /// Colored half-block characters, which every terminal can show.
    Halfblocks,
}

/// The read receipt type the matrix adapter sends (`ui.read_receipts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReceiptKind {
    /// `m.read`, which other users see.
    #[default]
    Public,
    /// `m.read.private`, which only the user's own devices see.
    Private,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// How long a prefix of a longer binding waits for more keys.
    pub timeoutlen_ms: u64,
    pub room_sort: RoomSort,
    /// Whether the room list is shown at startup.
    pub room_list_visible: bool,
    pub image_preview: ImagePreview,
    pub read_receipts: ReceiptKind,
}

impl Default for Settings {
    /// The design's defaults: `timeoutlen` 1000 ms, rooms by recent activity,
    /// the room list visible, image previews on, public receipts.
    fn default() -> Settings {
        Settings {
            timeoutlen_ms: 1000,
            room_sort: RoomSort::Recent,
            room_list_visible: true,
            image_preview: ImagePreview::Auto,
            read_receipts: ReceiptKind::Public,
        }
    }
}
