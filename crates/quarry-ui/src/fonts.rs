//! Bundled typefaces (both SIL OFL, licences next to the files).
//!
//! Embedding means the look is identical on every machine and a missing system
//! font can never silently change metrics. IBM Plex Sans is the UI face,
//! Lilex the buffer/grid face.

use gpui_kit::App;
use std::borrow::Cow;

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Italic.ttf"),
    include_bytes!("../assets/fonts/Lilex-Regular.ttf"),
    include_bytes!("../assets/fonts/Lilex-Medium.ttf"),
    include_bytes!("../assets/fonts/Lilex-Bold.ttf"),
    include_bytes!("../assets/fonts/Lilex-Italic.ttf"),
];

pub(crate) fn register(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = FONTS.iter().map(|b| Cow::Borrowed(*b)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("quarry-ui: could not register bundled fonts: {e}");
    }
}
