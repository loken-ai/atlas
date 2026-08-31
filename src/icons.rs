//! The SVG icon set, bundled into the binary.
//!
//! Icons, not characters. A glyph like a filled circle or a diamond renders in whatever font
//! the system happens to resolve it in - a different weight, a different baseline, sometimes a
//! colour emoji, and sometimes the missing-glyph box. These are 16x16 line art on a white
//! stroke, which is what makes egui's tint work: the tint multiplies, so white takes the
//! requested colour exactly and any other stroke would darken it.
//!
//! Rendering goes through egui_extras' SVG loader, which needs
//! `egui_extras::install_image_loaders` called once - without it every icon draws as the
//! missing-image placeholder.

use eframe::egui;

/// One variant per file under `src/icons/`; the name is the file stem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Bolt,
    Check,
    Chart,
    Gear,
    Globe,
    Package,
    Refresh,
    Server,
    Thermometer,
    Warning,
}

impl Icon {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Bolt => include_bytes!("icons/bolt.svg"),
            Self::Check => include_bytes!("icons/check.svg"),
            Self::Chart => include_bytes!("icons/chart.svg"),
            Self::Gear => include_bytes!("icons/gear.svg"),
            Self::Globe => include_bytes!("icons/globe.svg"),
            Self::Package => include_bytes!("icons/package.svg"),
            Self::Refresh => include_bytes!("icons/refresh.svg"),
            Self::Server => include_bytes!("icons/server.svg"),
            Self::Thermometer => include_bytes!("icons/thermometer.svg"),
            Self::Warning => include_bytes!("icons/warning.svg"),
        }
    }

    /// `bytes://` tells the loader to take the bundled bytes rather than touch the filesystem.
    pub fn uri(self) -> &'static str {
        match self {
            Self::Bolt => "bytes://icons/bolt.svg",
            Self::Check => "bytes://icons/check.svg",
            Self::Chart => "bytes://icons/chart.svg",
            Self::Gear => "bytes://icons/gear.svg",
            Self::Globe => "bytes://icons/globe.svg",
            Self::Package => "bytes://icons/package.svg",
            Self::Refresh => "bytes://icons/refresh.svg",
            Self::Server => "bytes://icons/server.svg",
            Self::Thermometer => "bytes://icons/thermometer.svg",
            Self::Warning => "bytes://icons/warning.svg",
        }
    }

    pub fn image(self, size_pt: f32, tint: egui::Color32) -> egui::Image<'static> {
        egui::Image::new((self.uri(), self.bytes()))
            .fit_to_exact_size(egui::vec2(size_pt, size_pt))
            .tint(tint)
    }

    pub fn show(self, ui: &mut egui::Ui, size_pt: f32, tint: egui::Color32) -> egui::Response {
        ui.add(self.image(size_pt, tint))
    }

    pub const ALL: [Icon; 9] = [
        Icon::Bolt,
        Icon::Chart,
        Icon::Gear,
        Icon::Globe,
        Icon::Package,
        Icon::Refresh,
        Icon::Server,
        Icon::Thermometer,
        Icon::Warning,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant resolves to bytes that are an SVG, and to a distinct uri. A variant added
    /// without its file compiles and then draws a placeholder at runtime, which is exactly the
    /// failure this set exists to avoid.
    #[test]
    fn every_icon_carries_a_real_svg_under_its_own_uri() {
        let mut uris = std::collections::HashSet::new();
        for icon in Icon::ALL {
            let bytes = icon.bytes();
            let head = std::str::from_utf8(&bytes[..bytes.len().min(200)]).unwrap_or_default();
            assert!(head.contains("<svg"), "{icon:?} is not an SVG");
            // A white stroke is what makes the tint land on the requested colour: egui
            // multiplies, so anything darker comes out darker than asked for.
            let body = std::str::from_utf8(bytes).unwrap_or_default();
            assert!(
                body.contains("stroke=\"#ffffff\"") || body.contains("currentColor"),
                "{icon:?} strokes in neither white nor currentColor, so tinting darkens it"
            );
            assert!(
                uris.insert(icon.uri()),
                "{icon:?} shares a uri with another"
            );
        }
        assert_eq!(uris.len(), Icon::ALL.len());
    }
}
