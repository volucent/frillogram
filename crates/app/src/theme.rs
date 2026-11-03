//! Applies `[appearance]` to gpui-component's global theme.

use std::rc::Rc;

use frillogram_core::config::{Appearance, ThemeChoice};
use gpui_kit::{
    component::{Theme, ThemeMode, ThemeRegistry},
    *,
};

/// Sets up the theme once, before the first [`apply`].
pub(crate) fn init(appearance: &Appearance, cx: &mut App) {
    let Some(accent) = &appearance.accent else { return };
    // The accent goes into copies of the registered light and dark themes,
    // so it survives every mode change and hover and active shades derive
    // from it.
    let registry = ThemeRegistry::global(cx);
    let (mut light, mut dark) = ((**registry.default_light_theme()).clone(), (**registry.default_dark_theme()).clone());
    for config in [&mut light, &mut dark] {
        config.colors.primary = Some(accent.clone().into());
        config.colors.primary_hover = None;
        config.colors.primary_active = None;
        config.colors.primary_foreground = Some(foreground_on(accent).into());
    }
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    });
}

/// Applies the light or dark mode `appearance` asks for, following the
/// window's appearance for "system", and its fonts. Call it again when the
/// window's appearance changes.
pub(crate) fn apply(appearance: &Appearance, window: &Window, cx: &mut App) {
    let mode = match appearance.theme {
        ThemeChoice::System => window.appearance().into(),
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
    };
    Theme::change(mode, None, cx);
    // Loading the mode's theme can reset the fonts, so they're set after it.
    // The root view sizes rem units from `font_size`, so all text scales.
    Theme::update(cx, |theme| {
        theme.font_size = px(appearance.font_size as f32);
        if let Some(family) = &appearance.font_family {
            theme.font_family = family.clone().into();
        }
    });
}

/// Black or white, whichever reads better on the `#rrggbb` color `background`.
fn foreground_on(background: &str) -> &'static str {
    let channel = |i: usize| {
        let value = u8::from_str_radix(&background[i..i + 2], 16).unwrap_or(0) as f32 / 255.;
        // sRGB to linear, as WCAG's relative luminance asks.
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    // Where the contrast ratios against black and white are equal.
    if luminance > 0.179 { "#000000" } else { "#ffffff" }
}

#[cfg(test)]
mod tests {
    use super::foreground_on;

    #[test]
    fn accent_text_contrasts_with_the_accent() {
        assert_eq!(foreground_on("#2563eb"), "#ffffff");
        assert_eq!(foreground_on("#000000"), "#ffffff");
        assert_eq!(foreground_on("#FACC15"), "#000000");
        assert_eq!(foreground_on("#ffffff"), "#000000");
    }
}
