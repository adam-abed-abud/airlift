use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

/// Render text as a terminal QR code using half-block characters, which fits
/// a typical URL in about 20 lines. Dark-on-light is inverted here because
/// most terminals have dark backgrounds and phone cameras decode a code with
/// a light quiet zone far more reliably.
pub fn render(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}
