//! Shows how Kollate splits a markup's ink: `cargo run --example markup -- <id>.svg [<id>.jpg]`
use kollate_core::markup::{Page, segment};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let svg = std::fs::read_to_string(&args[0]).expect("markup SVG");
    let strokes = segment::strokes(&svg);
    let s = segment::segment(&strokes);
    println!("{} strokes", strokes.len());
    for n in &s.notes {
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
    for m in &s.marks {
        let crops = page.as_ref().map_or(0, |p| p.marked(m, &strokes).len());
        println!(
            "mark  {:?}  {} strokes  {crops} line crops",
            m.kind,
            m.strokes.len()
        );
    }
}
