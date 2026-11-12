//! The letter tile of a remote app with no icon: drawn on the Remote apps
//! page, and rendered as a PNG for the app's Dock, window, taskbar and Start
//! menu icons, so a separate app never shows Newport's icon.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode,
    Transform,
};

/// The corner radius, as a share of the side: 14 px of the page's 64.
pub const RADIUS: f32 = 14. / 64.;
/// The rendered PNG's side, in pixels.
pub const SIDE: u32 = 512;
const MAX_CACHED: usize = 256;

/// Initials of the first two words: "Visual Studio Code" is "VS".
pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters
    }
}

/// The letters' size, as a share of the side: 26 px of 64 for one, 22 for two.
pub fn letter_size(initials: &str) -> f32 {
    if initials.chars().count() > 1 {
        22. / 64.
    } else {
        26. / 64.
    }
}

/// The gradient's top and bottom colours as hue, saturation and lightness,
/// each from 0 to 1. The hue comes from the name, so an app keeps its colour.
pub fn gradient(name: &str, dark: bool) -> [(f32, f32, f32); 2] {
    let hash = name.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let hue = (hash % 360) as f32 / 360.;
    let lightness = if dark { 0.36 } else { 0.52 };
    [(hue, 0.5, lightness + 0.08), (hue, 0.5, lightness - 0.04)]
}

/// The tile in the page's light colours, as a PNG `SIDE` pixels square.
pub fn png(name: &str) -> Option<Arc<[u8]>> {
    static TILES: Mutex<Option<HashMap<String, Arc<[u8]>>>> = Mutex::new(None);
    if let Some(png) = TILES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|tiles| tiles.get(name))
    {
        return Some(png.clone());
    }
    let png: Arc<[u8]> = render(name)?.into();
    let mut tiles = TILES.lock().unwrap();
    let tiles = tiles.get_or_insert_with(HashMap::new);
    if tiles.len() >= MAX_CACHED {
        tiles.clear();
    }
    tiles.insert(name.to_owned(), png.clone());
    Some(png)
}

/// The app's own icon, else its tile.
pub fn icon_or_tile(icon: Option<Arc<[u8]>>, name: &str) -> Option<Arc<[u8]>> {
    icon.or_else(|| png(name))
}

fn render(name: &str) -> Option<Vec<u8>> {
    let side = SIDE as f32;
    let mut pixmap = Pixmap::new(SIDE, SIDE)?;
    let [top, bottom] = gradient(name, false);
    let shader = LinearGradient::new(
        Point::from_xy(0., 0.),
        Point::from_xy(0., side),
        vec![
            GradientStop::new(0., color(top)),
            GradientStop::new(1., color(bottom)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )?;
    let tile = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(
        &rounded_square(side, side * RADIUS)?,
        &tile,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let letters = initials(name);
    if let Some(path) = letters_path(&letters, side * letter_size(&letters), side) {
        let mut white = Paint::default();
        white.set_color(Color::WHITE);
        white.anti_alias = true;
        pixmap.fill_path(
            &path,
            &white,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    pixmap.encode_png().ok()
}

fn color((hue, saturation, lightness): (f32, f32, f32)) -> Color {
    let chroma = (1. - (2. * lightness - 1.).abs()) * saturation;
    let sector = hue.rem_euclid(1.) * 6.;
    let x = chroma * (1. - (sector % 2. - 1.).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.),
        1 => (x, chroma, 0.),
        2 => (0., chroma, x),
        3 => (0., x, chroma),
        4 => (x, 0., chroma),
        _ => (chroma, 0., x),
    };
    let m = lightness - chroma / 2.;
    Color::from_rgba(r + m, g + m, b + m, 1.).unwrap_or(Color::BLACK)
}

fn rounded_square(side: f32, radius: f32) -> Option<tiny_skia::Path> {
    // Control points that make a quarter circle from a cubic.
    let k = radius * (1. - 0.552_284_8);
    let far = side - radius;
    let mut path = PathBuilder::new();
    path.move_to(radius, 0.);
    path.line_to(far, 0.);
    path.cubic_to(side - k, 0., side, k, side, radius);
    path.line_to(side, far);
    path.cubic_to(side, side - k, side - k, side, far, side);
    path.line_to(radius, side);
    path.cubic_to(k, side, 0., side - k, 0., far);
    path.line_to(0., radius);
    path.cubic_to(0., k, k, 0., radius, 0.);
    path.close();
    path.finish()
}

/// The UI fonts first, semibold as the page draws the letters, then fonts
/// for letters they lack.
fn candidates() -> Vec<(PathBuf, u32)> {
    let mut fonts = Vec::new();
    if cfg!(target_os = "macos") {
        for file in [
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        ] {
            fonts.push((PathBuf::from(file), 0));
        }
    } else if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")
            .or_else(|| std::env::var_os("WINDIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        for file in [
            "SegUIVar.ttf",
            "segoeuisb.ttf",
            "segoeui.ttf",
            "arial.ttf",
            "msyh.ttc",
            "YuGothM.ttc",
            "malgun.ttf",
        ] {
            fonts.push((root.join("Fonts").join(file), 0));
        }
    } else {
        for file in [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
        ] {
            fonts.push((PathBuf::from(file), 0));
        }
    }
    fonts
}

/// A candidate font file's bytes: not read yet, or read (None when it isn't there).
type FontSlot = Option<Option<Arc<Vec<u8>>>>;

/// Each candidate font file, read once, None when it isn't there.
fn font_data(index: usize, path: &std::path::Path) -> Option<Arc<Vec<u8>>> {
    static FONTS: Mutex<Vec<FontSlot>> = Mutex::new(Vec::new());
    let mut fonts = FONTS.lock().unwrap();
    if fonts.len() <= index {
        fonts.resize(index + 1, None);
    }
    fonts[index]
        .get_or_insert_with(|| std::fs::read(path).ok().map(Arc::new))
        .clone()
}

/// The letters' outlines, centred on the tile with their capitals' height
/// centred vertically.
fn letters_path(letters: &str, size: f32, side: f32) -> Option<tiny_skia::Path> {
    let candidates = candidates();
    let mut glyphs = Vec::new();
    for letter in letters.chars() {
        let found = candidates
            .iter()
            .enumerate()
            .find_map(|(index, (path, face))| {
                let data = font_data(index, path)?;
                let glyph = ttf_parser::Face::parse(&data, *face)
                    .ok()?
                    .glyph_index(letter)?;
                Some((data, *face, glyph))
            });
        glyphs.extend(found);
    }
    let (first, first_face, _) = glyphs.first()?.clone();
    // Optical size as the page's letters have it, in its 64 px tile.
    let optical = size * 64. / side;
    let face = |data, index| styled(data, index, optical);
    let reference = face(&first, first_face)?;
    let scale = size / f32::from(reference.units_per_em());
    let cap = reference
        .capital_height()
        .map(f32::from)
        .unwrap_or(f32::from(reference.units_per_em()) * 0.7)
        * scale;
    let advances: Vec<f32> = glyphs
        .iter()
        .map(|(data, index, glyph)| {
            let face = face(data, *index)?;
            let scale = size / f32::from(face.units_per_em());
            Some(f32::from(face.glyph_hor_advance(*glyph)?) * scale)
        })
        .collect::<Option<_>>()?;
    let mut x = (side - advances.iter().sum::<f32>()) / 2.;
    let baseline = (side + cap) / 2.;
    let mut builder = Outline {
        path: PathBuilder::new(),
        scale,
        x,
        baseline,
    };
    for ((data, index, glyph), advance) in glyphs.iter().zip(advances) {
        let face = face(data, *index)?;
        builder.scale = size / f32::from(face.units_per_em());
        builder.x = x;
        face.outline_glyph(*glyph, &mut builder);
        x += advance;
    }
    builder.path.finish()
}

/// Semibold, at the given optical size, in a variable font.
fn styled(data: &[u8], index: u32, optical: f32) -> Option<ttf_parser::Face<'_>> {
    let mut face = ttf_parser::Face::parse(data, index).ok()?;
    for axis in face.variation_axes() {
        let value = match &axis.tag.to_bytes() {
            b"wght" => 600.,
            b"opsz" => optical,
            _ => continue,
        };
        face.set_variation(axis.tag, value.clamp(axis.min_value, axis.max_value));
    }
    Some(face)
}

/// Font units to pixels, with y pointing down.
struct Outline {
    path: PathBuilder,
    scale: f32,
    x: f32,
    baseline: f32,
}

impl Outline {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.baseline - y * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.path.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.path.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.point(x1, y1);
        let (x, y) = self.point(x, y);
        self.path.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.point(x1, y1);
        let (x2, y2) = self.point(x2, y2);
        let (x, y) = self.point(x, y);
        self.path.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_are_the_first_two_words_letters() {
        assert_eq!(initials("Visual Studio Code"), "VS");
        assert_eq!(initials("gedit"), "G");
        assert_eq!(initials("QTerminal drop down"), "QD");
        assert_eq!(initials("  (beta) tool"), "BT");
        assert_eq!(initials("--"), "?");
        assert_eq!(initials(""), "?");
        assert_eq!(letter_size("VS"), 22. / 64.);
        assert_eq!(letter_size("G"), 26. / 64.);
    }

    #[test]
    fn an_app_without_an_icon_gets_its_tile() {
        let own: Arc<[u8]> = Arc::from(&b"png"[..]);
        assert!(Arc::ptr_eq(
            &icon_or_tile(Some(own.clone()), "XTerm").unwrap(),
            &own
        ));
        assert_eq!(icon_or_tile(None, "XTerm"), png("XTerm"));
    }

    #[test]
    fn a_name_keeps_its_colour() {
        assert_eq!(gradient("XTerm", false), gradient("XTerm", false));
        assert_ne!(
            gradient("XTerm", false)[0].0,
            gradient("QTerminal", false)[0].0
        );
        let [top, bottom] = gradient("XTerm", true);
        assert!(top.2 > bottom.2);
        let red = color((0., 1., 0.5)).to_color_u8();
        assert_eq!((red.red(), red.green(), red.blue()), (255, 0, 0));
    }

    #[test]
    fn the_tile_renders_as_a_rounded_square_with_white_letters() {
        let png = png("Rxvt Color Unicode Terminal").unwrap();
        assert!(Arc::ptr_eq(
            &png,
            &super::png("Rxvt Color Unicode Terminal").unwrap()
        ));
        assert_eq!(render("Rxvt Color Unicode Terminal").unwrap(), png.to_vec());
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (SIDE, SIDE));
        assert_eq!(image.get_pixel(0, 0)[3], 0, "rounded corner");
        let edge = image.get_pixel(4, SIDE / 2);
        assert_eq!(edge[3], 255);
        assert!(edge[0] < 250 || edge[1] < 250 || edge[2] < 250, "coloured");
        let white = image
            .pixels()
            .filter(|p| p[0] > 250 && p[1] > 250 && p[2] > 250 && p[3] == 255)
            .count();
        // Two letters cover a few percent of the tile.
        assert!(white > (SIDE * SIDE / 50) as usize, "{white} white pixels");
    }
}
