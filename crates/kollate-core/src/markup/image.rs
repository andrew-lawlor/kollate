//! Pixels for the reader: notes rendered on their own, and the printed lines
//! under a mark cropped from the Kobo's page image.

use super::segment::{Bounds, Mark, MarkKind, Note, Rotation, Stroke};
use crate::Result;

/// 8-bit RGB pixels, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbImage {
    fn from_gray(width: u32, height: u32, gray: &[u8]) -> Self {
        Self {
            width,
            height,
            data: gray.iter().flat_map(|&g| [g, g, g]).collect(),
        }
    }
}

fn err(e: impl std::fmt::Display) -> crate::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()).into()
}

/// A note's ink alone, black on white, turned upright, with `pad` pixels of
/// margin. Other ink on the page (and a circle drawn around the note) is left
/// out, so the model sees only this writing.
pub fn render_note(svg: &str, strokes: &[Stroke], note: &Note, pad: u32) -> Result<RgbImage> {
    let b = note.bounds;
    let (width, height) = (
        b.width().ceil() as u32 + 2 * pad,
        b.height().ceil() as u32 + 2 * pad,
    );
    let head = svg.find("<g").map_or(svg, |i| &svg[..i]);
    let paths: String = note
        .strokes
        .iter()
        .map(|&i| strokes[i].xml.as_str())
        .collect();
    let doc = format!("{head}<g fill=\"#000000\" stroke=\"none\">{paths}</g></svg>");
    let tree = resvg::usvg::Tree::from_str(&doc, &resvg::usvg::Options::default()).map_err(err)?;
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(width, height).ok_or_else(|| err("empty note"))?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    let shift =
        resvg::tiny_skia::Transform::from_translate(pad as f32 - b.left, pad as f32 - b.top);
    resvg::render(&tree, shift, &mut pixmap.as_mut());
    let gray: Vec<u8> = pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0])
        .collect();
    Ok(rotate(width, height, &gray, note.rotation))
}

fn rotate(width: u32, height: u32, gray: &[u8], rotation: Rotation) -> RgbImage {
    let (w, h) = (width as usize, height as usize);
    let turned: Vec<u8> = match rotation {
        Rotation::None => return RgbImage::from_gray(width, height, gray),
        // Output is h wide and w tall; (x, y) in it comes from the source.
        Rotation::Anticlockwise => (0..w)
            .flat_map(|y| (0..h).map(move |x| gray[x * w + (w - 1 - y)]))
            .collect(),
        Rotation::Clockwise => (0..w)
            .flat_map(|y| (0..h).map(move |x| gray[(h - 1 - x) * w + y]))
            .collect(),
    };
    RgbImage::from_gray(height, width, &turned)
}

/// A markup as one picture: the Kobo's page with the ink drawn on it, cut to
/// `crop` (left, top, right, bottom in page pixels, inclusive), as a JPEG.
///
/// Drawn here rather than left to an SVG renderer: GNOME's newer image
/// loaders (glycin) placed an embedded page differently from librsvg, which
/// shifted the page under the ink.
pub fn compose_page(svg: &str, page_jpeg: &[u8], crop: Option<[i32; 4]>) -> Result<Vec<u8>> {
    let page = image::load_from_memory_with_format(page_jpeg, image::ImageFormat::Jpeg)
        .map_err(err)?
        .to_rgb8();
    let (pw, ph) = page.dimensions();
    let [left, top, right, bottom] = crop.unwrap_or([0, 0, pw as i32 - 1, ph as i32 - 1]);
    let x0 = left.clamp(0, pw as i32 - 1) as u32;
    let y0 = top.clamp(0, ph as i32 - 1) as u32;
    let w = (right.clamp(0, pw as i32 - 1) as u32 + 1)
        .saturating_sub(x0)
        .max(1);
    let h = (bottom.clamp(0, ph as i32 - 1) as u32 + 1)
        .saturating_sub(y0)
        .max(1);

    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).map_err(err)?;
    let scale = pw as f32 / tree.size().width();
    let mut ink = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| err("empty page"))?;
    let place = resvg::tiny_skia::Transform::from_scale(scale, scale)
        .post_translate(-(x0 as f32), -(y0 as f32));
    resvg::render(&tree, place, &mut ink.as_mut());

    let mut out = image::RgbImage::new(w, h);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let under = page.get_pixel(x0 + x, y0 + y).0;
        // Premultiplied: colour is already scaled by coverage.
        let over = ink.pixel(x, y).expect("in bounds");
        let keep = 255 - over.alpha() as u16;
        let mix = |p: u8, o: u8| ((p as u16 * keep) / 255 + o as u16).min(255) as u8;
        px.0 = [
            mix(under[0], over.red()),
            mix(under[1], over.green()),
            mix(under[2], over.blue()),
        ];
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode_image(&out)
        .map_err(err)?;
    Ok(jpeg)
}

/// The Kobo's page image (which has no ink on it), decoded.
pub struct Page {
    image: image::RgbImage,
    /// (top, bottom) of each printed line.
    lines: Vec<(u32, u32)>,
}

impl Page {
    pub fn from_jpeg(bytes: &[u8]) -> Result<Self> {
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
            .map_err(err)?
            .to_rgb8();
        let lines = text_lines(&image);
        Ok(Self { image, lines })
    }

    pub fn lines(&self) -> &[(u32, u32)] {
        &self.lines
    }

    /// Crops of the printed words a mark covers, one per line: the line an
    /// underline stroke sits under, or the lines inside a circle.
    pub fn marked(&self, mark: &Mark, strokes: &[Stroke]) -> Vec<RgbImage> {
        let mut regions = Vec::new();
        for &i in &mark.strokes {
            let b = strokes[i].bounds;
            let mid = (b.top + b.bottom) / 2.0;
            match mark.kind {
                MarkKind::Underline => {
                    let line = self
                        .lines
                        .iter()
                        .filter(|l| (l.0 as f32) < mid)
                        .min_by(|a, c| {
                            (a.1 as f32 - mid)
                                .abs()
                                .total_cmp(&(c.1 as f32 - mid).abs())
                        });
                    if let Some(&(top, bottom)) = line {
                        regions.push(Bounds {
                            left: b.left - 12.0,
                            top: top as f32 - 4.0,
                            right: b.right + 12.0,
                            bottom: bottom as f32 + 4.0,
                        });
                    }
                }
                MarkKind::Circle => {
                    for &(top, bottom) in &self.lines {
                        let middle = (top + bottom) as f32 / 2.0;
                        if (b.top..=b.bottom).contains(&middle) {
                            regions.push(Bounds {
                                left: b.left,
                                top: top as f32 - 4.0,
                                right: b.right,
                                bottom: bottom as f32 + 4.0,
                            });
                        }
                    }
                }
            }
        }
        regions.iter().filter_map(|r| self.crop(r)).collect()
    }

    fn crop(&self, r: &Bounds) -> Option<RgbImage> {
        let (w, h) = self.image.dimensions();
        let x0 = (r.left.max(0.0) as u32).min(w);
        let y0 = (r.top.max(0.0) as u32).min(h);
        let x1 = (r.right.max(0.0) as u32).min(w);
        let y1 = (r.bottom.max(0.0) as u32).min(h);
        (x1 > x0 && y1 > y0).then(|| {
            let view = image::imageops::crop_imm(&self.image, x0, y0, x1 - x0, y1 - y0).to_image();
            RgbImage {
                width: view.width(),
                height: view.height(),
                data: view.into_raw(),
            }
        })
    }
}

/// Printed lines, from the rows that contain dark pixels. Bands taller than a
/// line of text (illustrations) are skipped.
fn text_lines(image: &image::RgbImage) -> Vec<(u32, u32)> {
    let (w, h) = image.dimensions();
    let dark_row = |y: u32| {
        (0..w)
            .filter(|&x| {
                let p = image.get_pixel(x, y).0;
                (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 128
            })
            .count()
            > 3
    };
    let mut lines = Vec::new();
    let mut start = None;
    for y in 0..=h {
        match (y < h && dark_row(y), start) {
            (true, None) => start = Some(y),
            (false, Some(s)) => {
                if (8..=70).contains(&(y - s)) {
                    lines.push((s, y - 1));
                }
                start = None;
            }
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::super::segment::{segment, strokes};
    use super::*;

    #[test]
    fn renders_a_note_upright() {
        // An "L" written down the page: a long vertical stroke and a foot.
        let svg = "<svg width=\"1264\" height=\"1680\" viewBox=\"0 0 1264 1680\"><g>\
            <path d=\"M100,100 L110,100 L110,190 L100,190\"/>\
            <path d=\"M100,190 L140,190 L140,200 L100,200\"/>\
            <path d=\"M100,210 L110,210 L110,260 L100,260\"/></g></svg>";
        let ss = strokes(svg);
        let note = &segment(&ss).notes[0];
        assert_eq!(note.rotation, Rotation::Anticlockwise);
        let img = render_note(svg, &ss, note, 5).unwrap();
        // Turned: wider than tall, and ink (dark) is present.
        assert!(img.width > img.height, "{}x{}", img.width, img.height);
        assert_eq!(img.data.len() as u32, img.width * img.height * 3);
        assert!(img.data.iter().any(|&v| v < 64));
        assert!(img.data.contains(&255));
    }

    #[test]
    fn composes_the_ink_onto_the_page() {
        // A white 100x80 page, and ink: a black square at (60, 50)-(70, 60).
        let mut page = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut page, 95)
            .encode_image(&image::RgbImage::from_pixel(
                100,
                80,
                image::Rgb([255, 255, 255]),
            ))
            .unwrap();
        let svg = "<svg width=\"100\" height=\"80\" viewBox=\"0 0 100 80\">\
            <path d=\"M60,50 L70,50 L70,60 L60,60\"/></svg>";
        let decode = |bytes: &[u8]| image::load_from_memory(bytes).unwrap().to_luma8();

        let whole = decode(&compose_page(svg, &page, None).unwrap());
        assert_eq!(whole.dimensions(), (100, 80));
        assert!(whole.get_pixel(65, 55).0[0] < 40, "ink where it was drawn");
        assert!(whole.get_pixel(20, 20).0[0] > 215, "page elsewhere");

        // Cut to a band: the ink moves with the page, not on its own.
        let band = decode(&compose_page(svg, &page, Some([0, 40, 99, 79])).unwrap());
        assert_eq!(band.dimensions(), (100, 40));
        assert!(band.get_pixel(65, 15).0[0] < 40);
        assert!(band.get_pixel(65, 35).0[0] > 215);
    }

    #[test]
    fn rotates_pixels_correctly() {
        // 2x1: [dark, light] → anticlockwise puts the right pixel on top.
        let r = rotate(2, 1, &[0, 255], Rotation::Anticlockwise);
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(r.data, vec![255, 255, 255, 0, 0, 0]);
        let r = rotate(2, 1, &[0, 255], Rotation::Clockwise);
        assert_eq!(r.data, vec![0, 0, 0, 255, 255, 255]);
    }

    fn page_with_lines(lines: &[(u32, u32)]) -> Page {
        let mut img = image::RgbImage::from_pixel(400, 300, image::Rgb([255, 255, 255]));
        for &(top, bottom) in lines {
            for y in top..=bottom {
                for x in (20..380).step_by(3) {
                    img.put_pixel(x, y, image::Rgb([0, 0, 0]));
                }
            }
        }
        let lines = text_lines(&img);
        Page { image: img, lines }
    }

    #[test]
    fn finds_lines_and_crops_what_a_mark_covers() {
        // Two text lines, and an illustration-sized block that isn't a line.
        let page = page_with_lines(&[(20, 50), (95, 125), (150, 290)]);
        assert_eq!(page.lines(), &[(20, 50), (95, 125)]);

        let under = |l: f32, t: f32, r: f32| Stroke {
            xml: String::new(),
            points: vec![],
            bounds: Bounds {
                left: l,
                top: t,
                right: r,
                bottom: t + 10.0,
            },
        };
        let ss = vec![under(100.0, 128.0, 300.0), under(20.0, 52.0, 150.0)];
        let mark = Mark {
            kind: MarkKind::Underline,
            strokes: vec![1, 0],
        };
        let crops = page.marked(&mark, &ss);
        assert_eq!(crops.len(), 2);
        // Under the first line, from just before to just after the stroke.
        assert_eq!((crops[0].width, crops[0].height), (154, 38));
        assert_eq!((crops[1].width, crops[1].height), (224, 38));

        let circle = vec![Stroke {
            xml: String::new(),
            points: vec![],
            bounds: Bounds {
                left: 50.0,
                top: 80.0,
                right: 200.0,
                bottom: 140.0,
            },
        }];
        let mark = Mark {
            kind: MarkKind::Circle,
            strokes: vec![0],
        };
        let crops = page.marked(&mark, &circle);
        assert_eq!(crops.len(), 1);
        assert_eq!(crops[0].width, 150);
    }
}
