use ratatui::style::Color;

pub const BG: Color = Color::Rgb(0x0d, 0x11, 0x17);
pub const BG_ALT: Color = Color::Rgb(0x16, 0x1b, 0x22);
pub const FG: Color = Color::Rgb(0xc9, 0xd1, 0xd9);
pub const MUTED: Color = Color::Rgb(0x8b, 0x94, 0x9e);
pub const BORDER: Color = Color::Rgb(0x30, 0x36, 0x3d);
pub const ORANGE: Color = Color::Rgb(0xe3, 0xb3, 0x41);
pub const RED: Color = Color::Rgb(0xf8, 0x51, 0x49);

pub fn pending_border_color(frame_count: u64) -> Color {
    const COLORS: [Color; 5] = [ORANGE, RED, Color::Rgb(0x58, 0xa6, 0xff), Color::Rgb(0x3f, 0xb9, 0x50), Color::Rgb(0xbc, 0x8c, 0xff)];
    COLORS[((frame_count / 6) as usize) % COLORS.len()]
}
