//! Shows how Kollate splits a markup's ink:
//! `cargo run --example markup -- <id>.svg [<id>.jpg [<dir to save the notes and marked lines in, as the model sees them>]]`
use kollate_core::markup::{Page, RgbImage, image::render_note, segment};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let svg = std::fs::read_to_string(&args[0]).expect("markup SVG");
    let strokes = segment::strokes(&svg);
    let save = args.get(2).map(std::path::PathBuf::from);
    let write = |name: String, c: &RgbImage| {
        if let Some(dir) = &save {
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut bytes)
                .encode(&c.data, c.width, c.height, image::ExtendedColorType::Rgb8)
                .expect("encodes");
            std::fs::write(dir.join(name), bytes).expect("writes");
        }
    };
    let s = segment::segment(&strokes);
    println!("{} strokes", strokes.len());
    for (k, n) in s.notes.iter().enumerate() {
        write(
            format!("note{k}.jpg"),
            &render_note(&svg, &strokes, n, 16).expect("renders"),
        );
        let b = n.bounds;
        println!(
            "note  {} strokes  {:.0},{:.0}–{:.0},{:.0}  {:?}",
            n.strokes.len(),
            b.left,
            b.top,
            b.right,
            b.bottom,
            n.rotation
        );
    }
    let page = args
        .get(1)
        .map(|p| Page::from_jpeg(&std::fs::read(p).expect("page JPG")).expect("decodes"));

    for (n, m) in s.marks.iter().enumerate() {
        let crops = page.as_ref().map_or(Vec::new(), |p| p.marked(m, &strokes));
        for (i, c) in crops.iter().enumerate() {
            write(format!("mark{n}-{i}.jpg"), c);
        }
        let crops = crops.len();
        println!(
            "mark  {:?}  {} strokes  {crops} line crops",
            m.kind,
            m.strokes.len()
        );
    }
}
