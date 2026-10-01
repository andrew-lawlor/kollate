//! Reads an exported handwriting set (see kollate-core's handwriting-export)
//! with one model and one setup, and saves what was read for
//! `handwriting-score.py` to compare with the writers' answers.
//!
//! `cargo run --release --example handwriting-eval -- <set dir> <model.gguf> <vision.gguf>
//!     <name> [--setup kollate|no-names|no-grammar|whole-page] [--cpu] [--dictionaries <dir>]`
//!
//! Setups:
//! - `kollate`: what the app does (SPEC §8a).
//! - `no-names`: without correcting misread names from the book.
//! - `no-grammar`: without the Latin-script output grammar.
//! - `whole-page`: no splitting; the model gets the whole page with the ink
//!   on it and is asked for the handwriting.

use std::path::PathBuf;
use std::time::Instant;

use kollate_core::markup::{Context, Reader, RgbImage, read_page, transcribe};
use kollate_transcribe::Transcriber;
use serde_json::{Value, json};

const HANDWRITING: &str =
    "Transcribe the handwritten text in this image exactly. Output only the text.";
const PRINT: &str = "Transcribe the printed text in this image exactly. Output only the text.";
const WHOLE_PAGE: &str = "This is a page of a book with handwritten notes on it. Transcribe \
     only the handwriting, one note per line, exactly as written. Leave out the printed text. \
     Output only the handwriting.";

/// Reads like the app, optionally without the grammar, counting model calls.
struct Counted<'a> {
    model: &'a Transcriber,
    latin: bool,
    calls: usize,
}

impl Reader for Counted<'_> {
    fn handwriting(&mut self, image: &RgbImage) -> kollate_core::Result<String> {
        self.calls += 1;
        self.model.ask(image, HANDWRITING, self.latin, 160)
    }
    fn print(&mut self, image: &RgbImage) -> kollate_core::Result<String> {
        self.calls += 1;
        self.model.ask(image, PRINT, self.latin, 160)
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("handwriting-eval: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut positional = Vec::new();
    let mut setup = "kollate".to_owned();
    let mut cpu = false;
    let mut dictionaries = PathBuf::from("data/dictionaries");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--setup" => setup = args.next().ok_or("--setup needs a value")?,
            "--cpu" => cpu = true,
            "--dictionaries" => {
                dictionaries = args.next().ok_or("--dictionaries needs a dir")?.into()
            }
            _ => positional.push(arg),
        }
    }
    let [set, model, vision, name] = <[String; 4]>::try_from(positional).map_err(
        |_| "usage: handwriting-eval <set dir> <model.gguf> <vision.gguf> <name> [options]",
    )?;
    if !["kollate", "no-names", "no-grammar", "whole-page"].contains(&setup.as_str()) {
        return Err(format!("unknown setup {setup}").into());
    }
    let set = PathBuf::from(set);
    let items_dir = set.join("items");
    let manifest: Value = serde_json::from_slice(&std::fs::read(set.join("manifest.json"))?)?;
    let answers: Value = std::fs::read(set.join("answers.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);

    let dicts = kollate_core::dict::open_all(&[dictionaries]);
    if dicts.is_empty() {
        return Err("no dictionary found; pass --dictionaries".into());
    }
    let known = |w: &str| dicts.iter().any(|d| d.lookup(w).ok().flatten().is_some());

    let started = Instant::now();
    let transcriber = Transcriber::load(model.as_ref(), vision.as_ref(), !cpu)?;
    let load_ms = started.elapsed().as_millis();

    let mut results = serde_json::Map::new();
    let items = manifest["items"]
        .as_array()
        .ok_or("manifest has no items")?;
    for (n, item) in items.iter().enumerate() {
        let key = item["key"].as_str().ok_or("item without key")?;
        let answer = &answers["answers"][key];
        if answer["skip"].as_bool() == Some(true) {
            continue;
        }
        let notebook = item["kind"] == "notebook";
        let svg = std::fs::read_to_string(items_dir.join(format!("{key}.svg")))?;
        let page = std::fs::read(items_dir.join(format!("{key}.page.jpg"))).ok();
        let words: Option<Vec<String>> = std::fs::read(items_dir.join(format!("{key}.words.json")))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());

        let mut reader = Counted {
            model: &transcriber,
            latin: setup != "no-grammar",
            calls: 0,
        };
        let started = Instant::now();
        let read = match setup.as_str() {
            "whole-page" => {
                let view = image::load_from_memory(&std::fs::read(
                    items_dir.join(format!("{key}.view.jpg")),
                )?)?
                .to_rgb8();
                let img = RgbImage {
                    width: view.width(),
                    height: view.height(),
                    data: view.into_raw(),
                };
                reader.calls += 1;
                transcriber
                    .ask(&img, WHOLE_PAGE, true, 400)
                    .map(|note| json!({ "note": note }))
            }
            _ if notebook => read_page(&svg, &mut reader).map(|text| json!({ "note": text })),
            _ => {
                let names: Option<&dyn Fn(&str) -> bool> = (setup != "no-names").then_some(&known);
                transcribe(
                    &svg,
                    Context {
                        page_jpeg: page.as_deref(),
                        book_words: words.as_deref(),
                        known_word: names,
                    },
                    &mut reader,
                )
                .map(|t| json!({ "note": t.note, "text": t.text, "circled": t.circled }))
            }
        };
        let ms = started.elapsed().as_millis();
        let mut result = read.unwrap_or_else(|e| json!({ "error": e.to_string() }));
        result["ms"] = json!(ms);
        result["calls"] = json!(reader.calls);
        eprintln!("[{}/{}] {key} {ms} ms", n + 1, items.len());
        results.insert(key.to_owned(), result);
    }

    let out = set.join("results");
    std::fs::create_dir_all(&out)?;
    let device = if cpu { "cpu" } else { "gpu" };
    let path = out.join(format!("{name}__{setup}__{device}.json"));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "model": name,
            "setup": setup,
            "device": device,
            "load_ms": load_ms,
            "items": results,
        }))?,
    )?;
    println!("{}", path.display());
    Ok(())
}
