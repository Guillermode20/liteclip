#[cfg(windows)]
use image::{ImageBuffer, ImageFormat, Rgba};

#[cfg(windows)]
fn main() {
    use std::env;
    use std::path::PathBuf;

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let icon_path = out_dir.join("liteclip.ico");
    generate_icon(&icon_path).expect("Failed to generate app icon");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(
        icon_path
            .to_str()
            .expect("Icon path contains invalid UTF-16 sequence"),
    );
    res.compile().expect("Failed to compile Windows resources");
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn generate_icon(path: &std::path::Path) -> Result<(), image::ImageError> {
    let size = 256u32;
    let center = (size as f32 - 1.0) / 2.0;
    let outer_radius = size as f32 * 0.47;
    let inner_radius = size as f32 * 0.22;
    let outer_sq = outer_radius * outer_radius;
    let inner_sq = inner_radius * inner_radius;

    let mut image = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(size, size);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = x as f32 - center;
        let dy = y as f32 - center;
        let dist_sq = dx * dx + dy * dy;

        *pixel = if dist_sq <= inner_sq {
            Rgba([255, 255, 255, 255])
        } else if dist_sq <= outer_sq {
            Rgba([0, 0, 0, 255])
        } else {
            Rgba([0, 0, 0, 0])
        };
    }

    image.save_with_format(path, ImageFormat::Ico)
}
