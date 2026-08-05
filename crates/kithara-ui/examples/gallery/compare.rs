//! Compares two capture sets page by page and says, in numbers, where they
//! disagree.
//!
//! Enabled by `KITHARA_GALLERY_COMPARE=<a>:<b>:<out>`. It is only meaningful
//! when both sets were photographed at the same geometry — the capture writes
//! `frame.txt` beside the pages and this refuses to compare across a mismatch,
//! because two hosts scaled differently can be made to agree or disagree at
//! will.

use std::{
    env,
    fs::{File, create_dir_all},
    path::{Path, PathBuf},
};

use num_traits::cast::AsPrimitive;

use super::capture::{read_frame, write_png};

/// A channel difference below this is rasteriser noise: the two hosts run
/// different rasterisers, so antialiased edges never match bit for bit.
const NOISE: u8 = 24;

/// Runs the comparison when asked. Returns `false` when the environment
/// variable is absent, so the caller falls through.
pub(super) fn run() -> bool {
    let Some(spec) = env::var_os("KITHARA_GALLERY_COMPARE") else {
        return false;
    };
    match compare(&spec.to_string_lossy()) {
        Ok(pages) => println!("{pages} page(s) compared"),
        Err(error) => eprintln!("compare failed: {error}"),
    }
    true
}

fn compare(spec: &str) -> Result<usize, String> {
    let parts = spec.split(':').collect::<Vec<_>>();
    let [left, right, out] = parts.as_slice() else {
        return Err("expected KITHARA_GALLERY_COMPARE=<a>:<b>:<out>".to_owned());
    };
    let (left, right, out) = (Path::new(left), Path::new(right), PathBuf::from(out));
    match (read_frame(left), read_frame(right)) {
        (Some(a), Some(b)) if a != b => {
            return Err(format!(
                "the two sets were photographed differently — {}x{} at {}x versus {}x{} at {}x; \
                 recapture the second into the first's directory so it inherits the geometry",
                a.width, a.height, a.scale, b.width, b.height, b.scale
            ));
        }
        (None, _) | (_, None) => {
            return Err("a capture set has no frame.txt, so its geometry is unknown".to_owned());
        }
        _ => {}
    }
    create_dir_all(&out).map_err(|error| format!("create {}: {error}", out.display()))?;

    let mut pages = 0;
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(left).map_err(|error| format!("read {left:?}: {error}"))? {
        let path = entry
            .map_err(|error| format!("read {left:?}: {error}"))?
            .path();
        if path.extension().is_none_or(|ext| ext != "png") {
            continue;
        }
        let Some(name) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            continue;
        };
        let twin = right.join(&name);
        if !twin.exists() {
            rows.push((name, None));
            continue;
        }
        let a = read_png(&path)?;
        let b = read_png(&twin)?;
        if a.width != b.width || a.height != b.height {
            return Err(format!(
                "{name}: {}x{} against {}x{}",
                a.width, a.height, b.width, b.height
            ));
        }
        let (share, mask) = difference(&a, &b);
        write_png(&out.join(&name), &mask, a.width, a.height)?;
        rows.push((name, Some(share)));
        pages += 1;
    }

    rows.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!("page                          differing pixels");
    for (name, share) in &rows {
        match share {
            Some(share) => println!("{name:<30}{:>15.1}%", share * 100.0),
            None => println!("{name:<30}{:>16}", "missing"),
        }
    }
    println!(
        "\ndifference masks in {}; a channel gap under {NOISE} is treated as rasteriser noise, \
         because the two hosts rasterise with different engines",
        out.display()
    );
    Ok(pages)
}

struct Image {
    height: u32,
    rgba: Vec<u8>,
    width: u32,
}

fn read_png(path: &Path) -> Result<Image, String> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("decode {}: {error}", path.display()))?;
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut rgba)
        .map_err(|error| format!("decode {}: {error}", path.display()))?;
    rgba.truncate(info.buffer_size());
    Ok(Image {
        height: info.height,
        rgba,
        width: info.width,
    })
}

/// The share of pixels that differ beyond rasteriser noise, and a mask that
/// paints those pixels so the eye can find them.
fn difference(left: &Image, right: &Image) -> (f64, Vec<u8>) {
    let mut mask = vec![0; left.rgba.len()];
    let mut differing = 0_usize;
    let total = left.rgba.len() / 4;
    for (index, (a, b)) in left
        .rgba
        .chunks_exact(4)
        .zip(right.rgba.chunks_exact(4))
        .enumerate()
    {
        let gap = a
            .iter()
            .zip(b)
            .take(3)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        let at = index * 4;
        if gap > NOISE {
            differing += 1;
            mask[at] = 255;
            mask[at + 1] = 32;
            mask[at + 2] = 96;
            mask[at + 3] = 255;
        } else {
            let grey = u8::try_from(u16::from(a[0]) / 6).unwrap_or(0);
            mask[at] = grey;
            mask[at + 1] = grey;
            mask[at + 2] = grey;
            mask[at + 3] = 255;
        }
    }
    let share = if total == 0 {
        0.0
    } else {
        AsPrimitive::<f64>::as_(differing) / AsPrimitive::<f64>::as_(total)
    };
    (share, mask)
}
