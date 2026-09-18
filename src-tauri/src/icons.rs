//! Resolving an [`IconState`] to the bytes of a tray image.
//!
//! Three things decide which file is used, in this order:
//!
//! 1. `[icon].states` remaps a state onto a symbol name, so
//!    `states = { failed = "octagon" }` means the `failed` state draws
//!    `octagon.png`. A state with no entry uses its own name, which is also
//!    what `IconState::as_str()` returns and what the builtin files are called.
//! 2. `[icon].theme` is either `builtin` or a directory of overrides. A file
//!    missing from a theme directory falls back to the builtin, so a theme may
//!    replace one glyph without shipping eight.
//! 3. `[icon].mode` picks the set: `template` is black-on-transparent for
//!    macOS, `color` is filled for Linux, `auto` is per platform.

use std::path::Path;

use bridgewatch_core::config::IconConfig;
use bridgewatch_core::verdict::IconState;

/// The eight state names plus the eight symbol names, so a `[icon].states`
/// remap resolves without the user having to supply a file.
///
/// ⚠ Literals, because `include_bytes!` takes nothing else, which makes this a
/// second copy of the core's `BUILTIN_GLYPHS`. The test
/// `the_builtin_set_is_exactly_the_cores_glyph_list` fails if they diverge.
macro_rules! builtin_set {
    ($($name:literal),* $(,)?) => {
        /// Black on transparent: macOS renders these with `icon_as_template`.
        const TEMPLATE: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../icons/tray/template/", $name, ".png")))),*
        ];
        /// Filled: Linux's AppIndicator has no template concept.
        const COLOR: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../icons/tray/color/", $name, ".png")))),*
        ];
    };
}

builtin_set!(
    // IconState::as_str(), all eight.
    "unknown",
    "failed",
    "deployed_with_failure",
    "deployed",
    "running",
    "canceled",
    "parked_gate",
    "succeeded_no_deploy",
    // The symbol names `[icon].states` may point at.
    "question",
    "octagon",
    "triangle",
    "check",
    "check-outline",
    "arrows",
    "slash",
    "hourglass",
);

/// Which of the two builtin sets to draw from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Shape only; macOS inverts it for a dark menu bar.
    Template,
    /// Filled with the state's colour.
    Color,
}

impl Mode {
    /// Resolve `[icon].mode`. Anything unrecognised is treated as `auto`
    /// rather than refused: an icon is not worth failing to start over.
    pub fn resolve(configured: &str) -> Self {
        match configured {
            "template" => Mode::Template,
            "color" => Mode::Color,
            // `auto`: macOS is the only platform with a template convention.
            _ if cfg!(target_os = "macos") => Mode::Template,
            _ => Mode::Color,
        }
    }

    /// True when the tray image should be handed to macOS as a template.
    pub fn is_template(self) -> bool {
        self == Mode::Template
    }
}

/// One resolved tray image.
pub struct TrayImage {
    /// PNG bytes.
    pub bytes: Vec<u8>,
    /// Whether macOS should treat it as a template image.
    pub template: bool,
}

/// The symbol name a state draws, after `[icon].states`.
pub fn symbol_for(icon: &IconConfig, state: IconState) -> String {
    icon.states
        .get(state.as_str())
        .cloned()
        .unwrap_or_else(|| state.as_str().to_string())
}

/// Resolve the image for a state.
///
/// Never fails: an unreadable theme file or an unknown symbol falls back to the
/// builtin `unknown` glyph, because a tray with no icon at all is worse than a
/// tray with the wrong one.
pub fn image_for(icon: &IconConfig, state: IconState) -> TrayImage {
    let mode = Mode::resolve(&icon.mode);
    let symbol = symbol_for(icon, state);

    if icon.theme != "builtin" && !icon.theme.trim().is_empty() {
        let dir = bridgewatch_core::config::expand_tilde(Path::new(&icon.theme));
        // `<symbol>.png` first, then the 1x name, which is what a theme built
        // from the committed assets is most likely to contain.
        for candidate in [format!("{symbol}.png"), format!("{symbol}@1x.png")] {
            if let Ok(bytes) = std::fs::read(dir.join(&candidate)) {
                return TrayImage {
                    bytes,
                    template: mode.is_template(),
                };
            }
        }
        tracing::debug!(theme = %icon.theme, symbol, "no theme override; using the builtin");
    }

    let set = match mode {
        Mode::Template => TEMPLATE,
        Mode::Color => COLOR,
    };
    let bytes = set
        .iter()
        .find(|(name, _)| *name == symbol)
        .or_else(|| set.iter().find(|(name, _)| *name == "unknown"))
        .map(|(_, bytes)| bytes.to_vec())
        .unwrap_or_default();

    TrayImage {
        bytes,
        template: mode.is_template(),
    }
}

/// A tray image that has been decoded and checked to draw something.
pub struct Drawable {
    /// Decoded RGBA, ready for the tray.
    pub image: tauri::image::Image<'static>,
    /// Whether macOS should treat it as a template image.
    pub template: bool,
    /// Set when the image asked for could not be drawn and the builtin
    /// `unknown` glyph stands in for it: the symbol that failed.
    pub fell_back_from: Option<String>,
}

/// Resolve a state to an image the tray can ALWAYS draw.
///
/// ⛔ The tray must never be left without a visible image: a status item with
/// nothing drawable in it simply disappears from the menu bar while the
/// process keeps running, which reads as "bridgewatch quit". [`image_for`]
/// already falls back for a missing file, but not for a file that exists and
/// is not a PNG, or one that decodes to nothing visible (zero size, every
/// pixel transparent). Those fall back here to the builtin `unknown` glyph of
/// the same mode, and if even that were unusable, to a solid square.
pub fn drawable_for(icon: &IconConfig, state: IconState) -> Drawable {
    let resolved = image_for(icon, state);
    if let Some(image) = decode_visible(&resolved.bytes) {
        return Drawable {
            image,
            template: resolved.template,
            fell_back_from: None,
        };
    }
    let mode = Mode::resolve(&icon.mode);
    let set = match mode {
        Mode::Template => TEMPLATE,
        Mode::Color => COLOR,
    };
    let image = set
        .iter()
        .find(|(name, _)| *name == "unknown")
        .and_then(|(_, bytes)| decode_visible(bytes))
        .unwrap_or_else(last_resort);
    Drawable {
        image,
        template: mode.is_template(),
        fell_back_from: Some(symbol_for(icon, state)),
    }
}

/// Decode PNG bytes, keeping the result only if it would draw something.
fn decode_visible(bytes: &[u8]) -> Option<tauri::image::Image<'static>> {
    let image = tauri::image::Image::from_bytes(bytes).ok()?;
    let (w, h) = (image.width() as usize, image.height() as usize);
    let sane = w > 0 && h > 0 && image.rgba().len() == w * h * 4;
    let visible = image.rgba().as_chunks::<4>().0.iter().any(|px| px[3] > 0);
    (sane && visible).then_some(image)
}

/// A solid 16x16 black square: drawable in both modes, and needs no decoder.
fn last_resort() -> tauri::image::Image<'static> {
    const SIDE: u32 = 16;
    let rgba = [0u8, 0, 0, 255].repeat((SIDE * SIDE) as usize);
    tauri::image::Image::new_owned(rgba, SIDE, SIDE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> IconConfig {
        IconConfig::default()
    }

    #[test]
    fn every_state_resolves_to_a_non_empty_image() {
        for state in [
            IconState::Unknown,
            IconState::Failed,
            IconState::DeployedWithFailure,
            IconState::Deployed,
            IconState::Running,
            IconState::Canceled,
            IconState::ParkedGate,
            IconState::SucceededNoDeploy,
        ] {
            for mode in ["template", "color", "auto"] {
                let mut c = cfg();
                c.mode = mode.to_string();
                let image = image_for(&c, state);
                assert!(
                    image.bytes.starts_with(b"\x89PNG"),
                    "{state:?} in {mode} mode is not a PNG"
                );
            }
        }
    }

    #[test]
    fn the_builtin_set_is_exactly_the_cores_glyph_list() {
        // ⛔ Two lists of the same names, in two crates, because `include_bytes!`
        // needs literals and the core cannot see the PNGs. The core's list is
        // what validation warns against; a glyph shipped here but missing
        // there is reported as a typo, and one listed there but missing here
        // passes validation and then draws the question mark.
        let shipped: Vec<&str> = TEMPLATE.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            shipped,
            bridgewatch_core::config::BUILTIN_GLYPHS,
            "src-tauri/src/icons.rs builtin_set! and core BUILTIN_GLYPHS disagree"
        );
        let color: Vec<&str> = COLOR.iter().map(|(name, _)| *name).collect();
        assert_eq!(shipped, color);
    }

    #[test]
    fn states_table_remaps_the_symbol() {
        let mut c = cfg();
        c.mode = "template".into();
        let plain = image_for(&c, IconState::Failed).bytes;
        c.states
            .insert("failed".into(), "check-outline".to_string());
        let remapped = image_for(&c, IconState::Failed).bytes;
        assert_ne!(plain, remapped, "[icon].states did not change the glyph");
    }

    #[test]
    fn an_unknown_symbol_falls_back_rather_than_producing_nothing() {
        let mut c = cfg();
        c.states.insert("failed".into(), "no-such-glyph".into());
        assert!(
            image_for(&c, IconState::Failed)
                .bytes
                .starts_with(b"\x89PNG")
        );
    }

    /// A valid 2x2 RGBA PNG whose every pixel is fully transparent.
    const TRANSPARENT_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60,
        0x40, 0x07, 0x00, 0x00, 0x12, 0x00, 0x01, 0x77, 0xf1, 0xfa, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    const ALL_STATES: [IconState; 8] = [
        IconState::Unknown,
        IconState::Failed,
        IconState::DeployedWithFailure,
        IconState::Deployed,
        IconState::Running,
        IconState::Canceled,
        IconState::ParkedGate,
        IconState::SucceededNoDeploy,
    ];

    fn unknown_glyph(mode: &str) -> Vec<u8> {
        let mut c = cfg();
        c.mode = mode.into();
        let bytes = image_for(&c, IconState::Unknown).bytes;
        decode_visible(&bytes).unwrap().rgba().to_vec()
    }

    #[test]
    fn every_state_draws_something_visible_in_every_mode() {
        // The live finding of 2026-09-18: the tray icon vanished from the menu
        // bar for twenty minutes while the process ran. Whatever the cause, no
        // state may resolve to an image with nothing to draw.
        for state in ALL_STATES {
            for mode in ["template", "color", "auto"] {
                let mut c = cfg();
                c.mode = mode.into();
                let d = drawable_for(&c, state);
                assert!(d.fell_back_from.is_none(), "{state:?}/{mode} fell back");
                assert!(
                    d.image.rgba().as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                    "{state:?}/{mode} is invisible"
                );
            }
        }
    }

    #[test]
    fn a_theme_file_that_does_not_draw_falls_back_to_the_unknown_glyph() {
        // `image_for` only falls back for a MISSING file. A theme file that is
        // not a PNG, or one that decodes to fully transparent pixels, used to
        // be handed to the tray as-is: the first was dropped with a warning
        // (keeping a stale image), the second drew an empty menu-bar slot.
        let dir = std::env::temp_dir().join(format!("bw-bad-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("failed.png"), b"not a png at all").unwrap();
        std::fs::write(dir.join("running.png"), TRANSPARENT_PNG).unwrap();
        assert!(
            Image::from_bytes(TRANSPARENT_PNG).is_ok(),
            "fixture must decode"
        );

        for mode in ["template", "color"] {
            let mut c = cfg();
            c.mode = mode.into();
            c.theme = dir.to_string_lossy().into_owned();
            for (state, symbol) in [
                (IconState::Failed, "failed"),
                (IconState::Running, "running"),
            ] {
                let d = drawable_for(&c, state);
                assert_eq!(d.fell_back_from.as_deref(), Some(symbol), "{mode}");
                assert_eq!(d.image.rgba(), unknown_glyph(mode).as_slice(), "{mode}");
                assert_eq!(d.template, mode == "template");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_resort_is_visible() {
        let img = last_resort();
        assert_eq!(img.rgba().len(), 16 * 16 * 4);
        assert!(img.rgba().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    use tauri::image::Image;

    #[test]
    fn a_theme_directory_with_no_matching_file_falls_back_to_the_builtin() {
        let mut c = cfg();
        c.theme = "/nonexistent/bridgewatch-theme".into();
        assert!(
            image_for(&c, IconState::Deployed)
                .bytes
                .starts_with(b"\x89PNG")
        );
    }
}
