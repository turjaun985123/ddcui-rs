//! Window colours and how a theme is chosen.
//!
//! The palettes are plain `0xRRGGBB` numbers so this module stays free of
//! GPUI types and can be tested without a window. Colours are picked for
//! contrast: text on its background is at least readable in both palettes, and
//! the accent is the one loud colour in the window.

/// The theme the window was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Follow the desktop.
    System,
    /// Light window.
    Light,
    /// Dark window.
    Dark,
}

impl Theme {
    /// Read a theme name, for `--theme` and `DDCUI_THEME`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "system" | "auto" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

/// The colours of one theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Window background.
    pub bg: u32,
    /// Monitor cards and panels.
    pub surface: u32,
    /// Raised part of a card, e.g. its header.
    pub surface_alt: u32,
    /// Title and status bars.
    pub header: u32,
    /// Hairline between cards and around the window.
    pub border: u32,
    /// Normal text.
    pub text: u32,
    /// Secondary text.
    pub text_dim: u32,
    /// The filled part of a level bar and other highlights.
    pub accent: u32,
    /// The empty part of a level bar.
    pub track: u32,
    /// Quick-set buttons and presets.
    pub chip: u32,
    /// Chips while hovered.
    pub chip_hover: u32,
    /// Background of the demo-mode banner.
    pub notice_bg: u32,
    /// Text of the demo-mode banner.
    pub notice_text: u32,
    /// A failed write.
    pub failed: u32,
    /// A write in progress.
    pub pending: u32,
}

impl Palette {
    /// The original dark window.
    pub const DARK: Palette = Palette {
        bg: 0x18181b,
        surface: 0x27272a,
        surface_alt: 0x1f1f23,
        header: 0x1f1f23,
        border: 0x3f3f46,
        text: 0xf4f4f5,
        text_dim: 0xa1a1aa,
        accent: 0x3b82f6,
        track: 0x0f0f11,
        chip: 0x3f3f46,
        chip_hover: 0x52525b,
        notice_bg: 0x422006,
        notice_text: 0xfcd34d,
        failed: 0xfca5a5,
        pending: 0xa1a1aa,
    };

    /// A light window for bright rooms.
    pub const LIGHT: Palette = Palette {
        bg: 0xf4f4f5,
        surface: 0xffffff,
        surface_alt: 0xe4e4e7,
        header: 0xe4e4e7,
        border: 0xd4d4d8,
        text: 0x18181b,
        text_dim: 0x52525b,
        accent: 0x2563eb,
        track: 0xd4d4d8,
        chip: 0xe4e4e7,
        chip_hover: 0xd4d4d8,
        notice_bg: 0xfef3c7,
        notice_text: 0x92400e,
        failed: 0xb91c1c,
        pending: 0x71717a,
    };

    /// The colours of a resolved theme.
    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self::LIGHT,
            // `System` only reaches this function when the desktop could not be
            // asked, which `choose` already turns into dark.
            Theme::System | Theme::Dark => Self::DARK,
        }
    }
}

/// Decide which theme to use.
///
/// An explicit choice wins, then `DDCUI_THEME`, and `system` follows what the
/// desktop reports; dark is the fallback so the window is never unreadable.
pub fn choose(explicit: Option<Theme>, env: Option<&str>, desktop_dark: Option<bool>) -> Theme {
    let requested = explicit
        .or_else(|| env.and_then(Theme::parse))
        .unwrap_or(Theme::System);
    match requested {
        Theme::System => match desktop_dark {
            Some(false) => Theme::Light,
            _ => Theme::Dark,
        },
        other => other,
    }
}

/// The palette for a choice, asking the desktop when the choice is `system`.
pub fn resolve(explicit: Option<Theme>, env: Option<&str>) -> Palette {
    Palette::for_theme(choose(explicit, env, desktop_prefers_dark()))
}

/// Whether the desktop asks for a dark colour scheme.
///
/// GNOME keeps this in `org.gnome.desktop.interface color-scheme`, where
/// `prefer-dark` means dark and `default` means "follow the wallpaper". A
/// missing `gsettings`, a missing key or an unreadable answer all return
/// `None`, which leaves the window dark.
pub fn desktop_prefers_dark() -> Option<bool> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim().trim_matches(['\'', '"']);
    if text.is_empty() {
        return None;
    }
    Some(text.contains("dark"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_are_read() {
        assert_eq!(Theme::parse("system"), Some(Theme::System));
        assert_eq!(Theme::parse("auto"), Some(Theme::System));
        assert_eq!(Theme::parse("light"), Some(Theme::Light));
        assert_eq!(Theme::parse(" Dark "), Some(Theme::Dark));
        assert_eq!(Theme::parse("sepia"), None);
        assert_eq!(Theme::parse(""), None);
    }

    #[test]
    fn the_first_choice_that_is_made_wins() {
        // An explicit flag beats the environment and the desktop.
        assert_eq!(
            choose(Some(Theme::Light), Some("dark"), Some(true)),
            Theme::Light
        );
        // The environment beats the desktop.
        assert_eq!(choose(None, Some("light"), Some(true)), Theme::Light);
        // A broken environment value is ignored.
        assert_eq!(choose(None, Some("sepia"), Some(true)), Theme::Dark);
    }

    #[test]
    fn system_follows_the_desktop_and_defaults_to_dark() {
        assert_eq!(choose(None, None, Some(false)), Theme::Light);
        assert_eq!(choose(None, None, Some(true)), Theme::Dark);
        assert_eq!(choose(None, None, None), Theme::Dark);
        assert_eq!(
            choose(Some(Theme::System), Some("light"), None),
            Theme::Dark
        );
    }

    #[test]
    fn both_palettes_are_usable() {
        let dark = Palette::for_theme(Theme::Dark);
        let light = Palette::for_theme(Theme::Light);
        for palette in [dark, light] {
            assert_ne!(palette.bg, palette.text);
            assert_ne!(palette.accent, palette.track);
            assert_ne!(palette.notice_bg, palette.notice_text);
            assert_ne!(palette.surface, palette.bg);
        }
        assert_ne!(dark.bg, light.bg);
        assert_eq!(Palette::for_theme(Theme::System), Palette::DARK);
    }
}
