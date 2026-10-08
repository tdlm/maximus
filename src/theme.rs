use ratatui::style::Color;
use two_face::theme::EmbeddedThemeName;

use crate::syntax::{self, Custom};

/// Where a theme's syntax colours come from.
#[derive(Debug, Clone, Copy)]
pub enum Syntax {
    Embedded(EmbeddedThemeName),
    Custom(&'static Custom),
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub id: &'static str,
    pub label: &'static str,
    pub bg: Color,
    /// Slightly raised surface for modals.
    pub surface: Color,
    pub fg: Color,
    pub muted: Color,
    pub border: Color,
    pub accent: Color,
    pub selection: Color,
    pub yellow: Color,
    pub red: Color,
    pub blue: Color,
    pub green: Color,
    pub add_bg: Color,
    pub del_bg: Color,
    pub syntax: Syntax,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The theme used when none is configured or the configured id is unknown.
pub const DEFAULT_ID: &str = "beardy-blueberry";

pub const THEMES: &[Theme] = &[
    Theme {
        id: "catppuccin-mocha",
        label: "Catppuccin Mocha",
        bg: rgb(0x1e1e2e),
        surface: rgb(0x24243a),
        fg: rgb(0xcdd6f4),
        muted: rgb(0x7f849c),
        border: rgb(0x45475a),
        accent: rgb(0xcba6f7),
        selection: rgb(0x363a4f),
        yellow: rgb(0xf9e2af),
        red: rgb(0xf38ba8),
        blue: rgb(0x89b4fa),
        green: rgb(0xa6e3a1),
        add_bg: rgb(0x26352f),
        del_bg: rgb(0x3b2632),
        syntax: Syntax::Embedded(EmbeddedThemeName::CatppuccinMocha),
    },
    Theme {
        id: "tokyo-night",
        label: "Tokyo Night",
        bg: rgb(0x1a1b26),
        surface: rgb(0x1f2335),
        fg: rgb(0xc0caf5),
        muted: rgb(0x737aa2),
        border: rgb(0x3b4261),
        accent: rgb(0x7aa2f7),
        selection: rgb(0x2e3c64),
        yellow: rgb(0xe0af68),
        red: rgb(0xf7768e),
        blue: rgb(0x7dcfff),
        green: rgb(0x9ece6a),
        add_bg: rgb(0x20303b),
        del_bg: rgb(0x37222c),
        syntax: Syntax::Embedded(EmbeddedThemeName::TwoDark),
    },
    Theme {
        id: "gruvbox-dark",
        label: "Gruvbox Dark",
        bg: rgb(0x282828),
        surface: rgb(0x32302f),
        fg: rgb(0xebdbb2),
        muted: rgb(0x928374),
        border: rgb(0x504945),
        accent: rgb(0xfe8019),
        selection: rgb(0x45403d),
        yellow: rgb(0xfabd2f),
        red: rgb(0xfb4934),
        blue: rgb(0x83a598),
        green: rgb(0xb8bb26),
        add_bg: rgb(0x32361a),
        del_bg: rgb(0x402120),
        syntax: Syntax::Embedded(EmbeddedThemeName::GruvboxDark),
    },
    Theme {
        id: "nord",
        label: "Nord",
        bg: rgb(0x2e3440),
        surface: rgb(0x343b49),
        fg: rgb(0xd8dee9),
        muted: rgb(0x7b88a1),
        border: rgb(0x4c566a),
        accent: rgb(0x88c0d0),
        selection: rgb(0x434c5e),
        yellow: rgb(0xebcb8b),
        red: rgb(0xbf616a),
        blue: rgb(0x81a1c1),
        green: rgb(0xa3be8c),
        add_bg: rgb(0x36433f),
        del_bg: rgb(0x453640),
        syntax: Syntax::Embedded(EmbeddedThemeName::Nord),
    },
    Theme {
        id: "beardy-blueberry",
        label: "Beardy Blueberry",
        bg: rgb(0x111422),
        surface: rgb(0x1a1e33),
        fg: rgb(0xbcc1dc),
        muted: rgb(0x6673b3),
        border: rgb(0x3c4776),
        accent: rgb(0x8eb0e6),
        selection: rgb(0x37435d),
        yellow: rgb(0xeacd61),
        red: rgb(0xe35535),
        blue: rgb(0x69c3ff),
        green: rgb(0x3cec85),
        add_bg: rgb(0x142c2c),
        del_bg: rgb(0x281a22),
        syntax: Syntax::Custom(&syntax::BEARDY_BLUEBERRY),
    },
    Theme {
        id: "catppuccin-latte",
        label: "Catppuccin Latte",
        bg: rgb(0xeff1f5),
        surface: rgb(0xe6e9ef),
        fg: rgb(0x4c4f69),
        muted: rgb(0x8c8fa1),
        border: rgb(0xbcc0cc),
        accent: rgb(0x8839ef),
        selection: rgb(0xccd0da),
        yellow: rgb(0xdf8e1d),
        red: rgb(0xd20f39),
        blue: rgb(0x1e66f5),
        green: rgb(0x40a02b),
        add_bg: rgb(0xd5ead3),
        del_bg: rgb(0xf3d3da),
        syntax: Syntax::Embedded(EmbeddedThemeName::CatppuccinLatte),
    },
];

pub fn by_id(id: &str) -> &'static Theme {
    THEMES
        .iter()
        .find(|t| t.id == id)
        .or_else(|| THEMES.iter().find(|t| t.id == DEFAULT_ID))
        .unwrap_or(&THEMES[0])
}

impl Theme {
    pub fn rgb_of(c: Color) -> (u8, u8, u8) {
        match c {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (0, 0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ids_fall_back_to_the_default_theme() {
        assert_eq!(by_id(DEFAULT_ID).id, DEFAULT_ID);
        assert_eq!(by_id("no-such-theme").id, DEFAULT_ID);
    }
}
