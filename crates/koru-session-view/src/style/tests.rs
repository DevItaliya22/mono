// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn terminal_palette_is_owned_by_the_captured_style() {
    let appearance = TerminalAppearance {
        dark: false,
        font: FontChoice::SourceCodePro,
        zoom: 1.25,
    };
    let epoch = appearance.epoch(1.5);
    let light = TerminalStyle {
        epoch,
        metrics: TextMetrics::new(14.0, 20.0),
        stack: appearance.font.stack(),
        cell_width: 8.0,
        cell_height: 20.0,
    };
    let dark = TerminalStyle {
        epoch: StyleEpoch {
            dark: true,
            ..epoch
        },
        ..light
    };
    assert_eq!(epoch.font, FontChoice::SourceCodePro);
    assert_eq!(f32::from_bits(epoch.zoom_bits), 1.25);
    assert_eq!(f32::from_bits(epoch.scale_bits), 1.5);
    let defaults = CellStyle::default();
    assert_eq!(
        light.resolve(defaults).background,
        light.palette().background
    );
    assert_eq!(dark.resolve(defaults).background, dark.palette().background);
    assert_ne!(light.resolve(defaults), dark.resolve(defaults));
    for index in 0..16 {
        let cell = CellStyle {
            foreground: TerminalColor::Indexed { index },
            ..defaults
        };
        assert_eq!(
            light.resolve(cell).foreground,
            light.palette().ansi[index as usize]
        );
        assert_eq!(
            dark.resolve(cell).foreground,
            dark.palette().ansi[index as usize]
        );
    }
}
