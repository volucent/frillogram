//! Applies `[appearance]` and `[colors]` to gpui-component's global theme.

use std::rc::Rc;

use frillogram_core::config::{Appearance, Colors, Palette, ThemeChoice};
use gpui_kit::{
    component::{ActiveTheme, Theme, ThemeConfig, ThemeConfigColors, ThemeMode, ThemeRegistry},
    *,
};

/// Sets up the light and dark themes from the palettes once, before the
/// first [`apply`].
pub(crate) fn init(appearance: &Appearance, colors: &Colors, cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let (mut light, mut dark) = ((**registry.default_light_theme()).clone(), (**registry.default_dark_theme()).clone());
    for (config, palette) in [(&mut light, &colors.light), (&mut dark, &colors.dark)] {
        paint(config, palette, appearance.accent.as_deref());
    }
    Theme::update(cx, |theme| {
        theme.light_theme = Rc::new(light);
        theme.dark_theme = Rc::new(dark);
    });
}

/// Replaces every color of `config` with ones from `palette`. The colors not
/// set here the theme derives from these, so none of the stock theme's
/// whites and blacks are left.
fn paint(config: &mut ThemeConfig, palette: &Palette, accent: Option<&str>) {
    let accent = accent.unwrap_or(&palette.accent);
    let p = palette;
    let mut colors = ThemeConfigColors::default();
    let set = |value: String| Some(value.into());
    colors.background = set(p.background.clone());
    colors.foreground = set(p.foreground.clone());
    colors.border = set(p.border.clone());
    colors.input = set(p.border.clone());
    colors.muted = set(mix(&p.background, &p.border, 0.5));
    colors.muted_foreground = set(p.muted.clone());
    colors.primary = set(accent.to_string());
    colors.primary_foreground = set(text_on(accent, p).to_string());
    colors.secondary = set(mix(&p.background, &p.border, 0.6));
    colors.secondary_foreground = set(p.foreground.clone());
    // Hover color of ghost buttons and list rows.
    colors.accent = set(mix(&p.background, &p.foreground, 0.07));
    colors.accent_foreground = set(p.foreground.clone());
    colors.list_hover = set(mix(&p.background, &p.foreground, 0.045));
    colors.popover = set(p.surface.clone());
    colors.popover_foreground = set(p.foreground.clone());
    colors.sidebar = set(p.sidebar.clone());
    colors.sidebar_foreground = set(p.foreground.clone());
    colors.sidebar_border = set(p.border.clone());
    colors.sidebar_accent = set(mix(&p.sidebar, &p.foreground, 0.07));
    colors.sidebar_accent_foreground = set(p.foreground.clone());
    colors.danger = set(p.danger.clone());
    colors.danger_foreground = set(text_on(&p.danger, p).to_string());
    colors.ring = set(accent.to_string());
    colors.caret = set(p.foreground.clone());
    colors.selection = set(format!("{accent}80"));
    colors.drop_target = set(format!("{accent}40"));
    colors.link = set(p.foreground.clone());
    colors.scrollbar = set(format!("{}00", p.background));
    colors.scrollbar_thumb = set(mix(&p.background, &p.foreground, 0.25));
    colors.scrollbar_thumb_hover = set(mix(&p.background, &p.foreground, 0.4));
    colors.skeleton = set(mix(&p.background, &p.border, 0.7));
    config.colors = colors;
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

/// The palette of the mode the theme is in now.
pub(crate) fn palette<'a>(colors: &'a Colors, cx: &App) -> &'a Palette {
    if cx.theme().mode.is_dark() { &colors.dark } else { &colors.light }
}

/// A validated `#rrggbb` as a GPUI color.
pub(crate) fn color(hex: &str) -> Hsla {
    rgb(u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0)).into()
}

/// The palette's foreground or background, whichever reads better on
/// `fill`: no pure black or white, even on the accent.
fn text_on<'a>(fill: &str, palette: &'a Palette) -> &'a str {
    let ratio = |text: &str| contrast(luminance(fill), luminance(text));
    if ratio(&palette.foreground) >= ratio(&palette.background) { &palette.foreground } else { &palette.background }
}

/// `a` moved toward `b` by `t` (0 to 1), as `#rrggbb`.
fn mix(a: &str, b: &str, t: f32) -> String {
    let (a, b) = (channels(a), channels(b));
    let mixed = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    format!("#{:02x}{:02x}{:02x}", mixed(0), mixed(1), mixed(2))
}

fn channels(hex: &str) -> [u8; 3] {
    let channel = |i: usize| hex.get(i..i + 2).and_then(|c| u8::from_str_radix(c, 16).ok()).unwrap_or(0);
    [channel(1), channel(3), channel(5)]
}

/// WCAG's relative luminance of `#rrggbb`.
fn luminance(hex: &str) -> f32 {
    let linear = |value: u8| {
        let value = value as f32 / 255.;
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    let [r, g, b] = channels(hex);
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG's contrast ratio of two luminances, 1 to 21.
fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use frillogram_core::config::Colors;

    use super::{contrast, luminance, mix, text_on};

    #[test]
    fn text_on_a_fill_is_the_palette_color_that_reads_better() {
        let colors = Colors::default();
        assert_eq!(text_on("#c5b6f2", &colors.light), colors.light.foreground);
        assert_eq!(text_on("#3d3a4b", &colors.light), colors.light.background);
        assert_eq!(text_on("#b3a4e8", &colors.dark), colors.dark.background);
    }

    #[test]
    fn mixing_moves_between_colors() {
        assert_eq!(mix("#000000", "#ffffff", 0.5), "#808080");
        assert_eq!(mix("#102030", "#102030", 0.3), "#102030");
        assert_eq!(mix("#f3eee6", "#3d3a4b", 0.), "#f3eee6");
    }

    #[test]
    fn default_palettes_avoid_pure_white_and_black_but_stay_readable() {
        for palette in [Colors::default().light, Colors::default().dark] {
            for color in [&palette.background, &palette.sidebar, &palette.surface, &palette.foreground] {
                assert!(!matches!(color.as_str(), "#ffffff" | "#000000"), "{color}");
            }
            let text = contrast(luminance(&palette.foreground), luminance(&palette.background));
            let muted = contrast(luminance(&palette.muted), luminance(&palette.background));
            // WCAG AA for body text and hints, without the glare of 21:1.
            assert!((7.0..15.0).contains(&text), "{text}");
            assert!(muted >= 4.0, "{muted}");
            for avatar in &palette.avatar_colors {
                assert!(contrast(luminance(avatar), luminance(&palette.avatar_text)) >= 4.5, "{avatar}");
            }
        }
    }
}
