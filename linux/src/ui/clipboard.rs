//! The system clipboard for images (X11 and Wayland), via arboard. Failing to reach it is never
//! an error: copy and paste fall back to the app's own clipboard.

use image::RgbaImage;

pub fn put_image(image: &RgbaImage) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let data = arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Borrowed(image.as_raw()),
        };
        let _ = clipboard.set_image(data);
    }
}

pub fn get_image() -> Option<RgbaImage> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let data = clipboard.get_image().ok()?;
    RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned())
}
