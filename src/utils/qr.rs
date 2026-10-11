use qrcode::render::unicode::Dense1x2;
use qrcode::{EcLevel, QrCode};

/// Render text as a terminal QR code using half-block characters.
///
/// Error-correction level L keeps the code one version smaller than the
/// default for a typical URL, which saves a few terminal lines; a screen
/// held at arm's length is a clean enough source that the extra redundancy
/// buys nothing. Dark-on-light is inverted because most terminals have
/// dark backgrounds and phone cameras want a light quiet zone.
pub fn render(text: &str) -> Option<String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::L).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}
