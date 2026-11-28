//! Screenshot baselines: app/tests/baselines/<name>.png, a pixel matches when every
//! channel is within ±2, a shot passes when 99% of its pixels match. `UPDATE_BASELINE=1` writes the baselines; a
//! failure writes the actual image and a diff (mismatches in red over a dimmed actual) to target/visual/.
use std::path::PathBuf;

use image::{Rgba, RgbaImage};

const TOLERANCE: i16 = 2;
const MIN_MATCH: f64 = 0.99;

fn baselines() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/baselines") }
fn out_dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/visual") }

/// Compares `img` with the baseline `name`; Err describes the mismatch.
pub fn check(name: &str, img: &RgbaImage) -> Result<(), String> {
    let path = baselines().join(format!("{name}.png"));
    if std::env::var_os("UPDATE_BASELINE").is_some() {
        std::fs::create_dir_all(baselines()).unwrap();
        img.save(&path).unwrap();
        println!("    baseline written: {}", path.display());
        return Ok(());
    }
    let Ok(base) = image::open(&path).map(|i| i.to_rgba8()) else {
        return Err(format!("no baseline {} (UPDATE_BASELINE=1 writes it)", path.display()));
    };
    let fail = |why: String, diff: Option<RgbaImage>| {
        std::fs::create_dir_all(out_dir()).unwrap();
        let actual = out_dir().join(format!("{name}.actual.png"));
        img.save(&actual).unwrap();
        if let Some(d) = diff { d.save(out_dir().join(format!("{name}.diff.png"))).unwrap() }
        Err(format!("{why}; actual and diff in {}", out_dir().display()))
    };
    if base.dimensions() != img.dimensions() {
        return fail(format!("size {:?}, baseline {:?}", img.dimensions(), base.dimensions()), None);
    }
    let mut diff = RgbaImage::new(img.width(), img.height());
    let mut matched = 0u64;
    for (x, y, a) in img.enumerate_pixels() {
        let b = base.get_pixel(x, y);
        let same = (0..4).all(|i| (a[i] as i16 - b[i] as i16).abs() <= TOLERANCE);
        if same { matched += 1 }
        diff.put_pixel(x, y, if same { Rgba([a[0] / 4, a[1] / 4, a[2] / 4, 255]) } else { Rgba([255, 0, 0, 255]) });
    }
    let ratio = matched as f64 / (img.width() as u64 * img.height() as u64) as f64;
    if ratio < MIN_MATCH { return fail(format!("{:.2}% of pixels match, need {:.0}%", ratio * 100., MIN_MATCH * 100.), Some(diff)) }
    Ok(())
}
