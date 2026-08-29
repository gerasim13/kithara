use std::{
    fs::{File, read_to_string, write},
    io::BufWriter,
    path::Path,
};

/// The pixel geometry one capture set was taken at.
///
/// Written beside the pages so another host can be photographed on exactly the
/// same terms, which is the only way a comparison between the two means
/// anything. A caller builds one, so its fields stay open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub height: u32,
    pub scale: f64,
    pub width: u32,
}

/// The name the first capture set was written with, and every set since.
const GEOMETRY_FILE: &str = "frame.txt";

/// Records the geometry a set was photographed at, beside the set.
///
/// # Errors
/// Fails when the file cannot be written.
pub fn write_geometry(dir: &Path, geometry: Geometry) -> Result<(), String> {
    let path = dir.join(GEOMETRY_FILE);
    write(
        &path,
        format!(
            "{} {} {}\n",
            geometry.width, geometry.height, geometry.scale
        ),
    )
    .map_err(|error| format!("write {}: {error}", path.display()))
}

/// Reads the geometry a capture set was taken at, if one was recorded.
#[must_use]
pub fn read_geometry(dir: &Path) -> Option<Geometry> {
    let text = read_to_string(dir.join(GEOMETRY_FILE)).ok()?;
    let mut parts = text.split_whitespace();
    Some(Geometry {
        width: parts.next()?.parse().ok()?,
        height: parts.next()?.parse().ok()?,
        scale: parts.next()?.parse().ok()?,
    })
}

/// Encodes tightly packed RGBA8 as a PNG.
///
/// # Errors
/// Fails when the file cannot be written or the pixels do not fill the frame.
pub fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let file = File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(rgba))
        .map_err(|error| format!("encode {}: {error}", path.display()))
}
