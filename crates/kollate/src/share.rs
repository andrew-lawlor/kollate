//! Quote cards: a highlight drawn as an image to share, and handing it to
//! the user's email app. Drawn here, with Pango and Cairo; nothing leaves
//! the computer unless the user sends it.

use std::ffi::{CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;

use gtk::gio::prelude::*;
use gtk::pango::prelude::*;
use gtk::{cairo, gio, glib, pango};
use kollate_core::store::Annotation;

/// How a card looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardOptions {
    /// 1080×1350 rather than square.
    pub tall: bool,
    pub dark: bool,
    /// Include the reader's note under the quote.
    pub note: bool,
    /// For a markup or notebook page, show the page with the ink.
    pub page: bool,
    /// A small "Kollate" at the foot.
    pub credit: bool,
}

const WIDTH: i32 = 1080;
const MARGIN: f64 = 100.0;
const SERIF: &str = "Literata";

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcConfigAppFontAddFile(config: *mut c_void, file: *const c_char) -> c_int;
}

/// Makes Literata available to cards: written to the cache once, then added
/// to fontconfig for this process. A fresh Pango font map sees it.
fn register_fonts() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let dir = glib::user_cache_dir().join("kollate").join("fonts");
        let fonts: [(&str, &[u8]); 2] = [
            ("Literata.ttf", include_bytes!("../data/fonts/Literata.ttf")),
            (
                "Literata-Italic.ttf",
                include_bytes!("../data/fonts/Literata-Italic.ttf"),
            ),
        ];
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        for (name, data) in fonts {
            let path = dir.join(name);
            let current = std::fs::metadata(&path).is_ok_and(|m| m.len() == data.len() as u64);
            if !current && std::fs::write(&path, data).is_err() {
                continue;
            }
            if let Ok(c) = CString::new(path.as_os_str().as_bytes()) {
                // SAFETY: a valid C string; a null config means the current one.
                unsafe { FcConfigAppFontAddFile(std::ptr::null_mut(), c.as_ptr()) };
            }
        }
    });
}

/// What a card quotes: the highlight's text, or for a markup with only a
/// note, the note; and the note, when there's text too.
pub fn quote(a: &Annotation) -> Option<(String, Option<String>)> {
    match (a.text(), a.note()) {
        (Some(text), note) => Some((text.to_owned(), note.map(str::to_owned))),
        (None, Some(note)) => Some((note.to_owned(), None)),
        (None, None) => None,
    }
}

type Rgb = (f64, f64, f64);

fn hex(s: u32) -> Rgb {
    (
        f64::from((s >> 16) & 0xff) / 255.0,
        f64::from((s >> 8) & 0xff) / 255.0,
        f64::from(s & 0xff) / 255.0,
    )
}

/// The highlight's Kobo colour (GNOME palette, a shade darker on light).
fn accent(a: &Annotation, dark: bool) -> Rgb {
    let palette: [u32; 4] = if dark {
        [0xf6d32d, 0xf36fb1, 0x62a0ea, 0x57e389]
    } else {
        [0xe5a50a, 0xe01b8f, 0x3584e4, 0x26a269]
    };
    match a.kind.as_str() {
        "highlight" | "note" => hex(palette[a.color.clamp(0, 3) as usize]),
        _ => hex(if dark { 0x9a9996 } else { 0x77767b }),
    }
}

fn set(cr: &cairo::Context, (r, g, b): Rgb) {
    cr.set_source_rgb(r, g, b);
}

fn layout(ctx: &pango::Context, text: &str, font: &str, size: f64, width: f64) -> pango::Layout {
    let l = pango::Layout::new(ctx);
    let mut desc = pango::FontDescription::from_string(font);
    desc.set_absolute_size(size * f64::from(pango::SCALE));
    l.set_font_description(Some(&desc));
    l.set_width((width * f64::from(pango::SCALE)) as i32);
    l.set_wrap(pango::WrapMode::WordChar);
    l.set_line_spacing(1.2);
    l.set_text(text);
    l
}

fn height(l: &pango::Layout) -> f64 {
    f64::from(l.pixel_size().1)
}

/// Draws the card for `a` as PNG bytes.
pub fn render(a: &Annotation, o: CardOptions) -> Result<Vec<u8>, String> {
    register_fonts();
    let Some((text, note)) = quote(a) else {
        return Err("Nothing to quote".into());
    };
    let note = note.filter(|_| o.note);
    let h = if o.tall { 1350 } else { 1080 };
    let (w, hf) = (f64::from(WIDTH), f64::from(h));
    let inner = w - 2.0 * MARGIN;
    let (bg, fg, muted) = if o.dark {
        (hex(0x1f1d24), hex(0xf2eee6), hex(0xa9a39a))
    } else {
        (hex(0xfbf8f1), hex(0x1c1b1a), hex(0x6b665e))
    };

    let surface =
        cairo::ImageSurface::create(cairo::Format::Rgb24, WIDTH, h).map_err(|e| e.to_string())?;
    let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    set(&cr, bg);
    cr.paint().map_err(|e| e.to_string())?;
    // A fresh font map, so the fonts registered above are seen.
    let fontmap = pangocairo::FontMap::new();
    let ctx = fontmap.create_context();
    pangocairo::functions::update_context(&cr, &ctx);

    // The rule, in the highlight's colour.
    let mut y = MARGIN;
    set(&cr, accent(a, o.dark));
    cr.rectangle(MARGIN, y, 72.0, 7.0);
    cr.fill().map_err(|e| e.to_string())?;
    y += 56.0;

    // The book at the foot, measured first: the rest of the card is for
    // the page, the quote and the note.
    let title = layout(
        &ctx,
        &a.book_title,
        &format!("{SERIF} SemiBold"),
        30.0,
        inner,
    );
    let author = a
        .book_author
        .as_deref()
        .filter(|_| a.kind != "page")
        .map(|au| layout(&ctx, au, &format!("{SERIF} Italic"), 26.0, inner));
    let foot = height(&title) + author.as_ref().map_or(0.0, |l| 6.0 + height(l));
    let bottom = hf - MARGIN - foot;

    // The page with the ink, for a markup or notebook page. A notebook
    // page's image is its content: it takes the whole card, without the
    // text repeated under it.
    let page = (o.page && matches!(a.kind.as_str(), "markup" | "page"))
        .then(|| a.markup_view())
        .flatten()
        .and_then(|p| {
            page_surface(&p)
                .inspect_err(|e| eprintln!("kollate: card page {}: {e}", p.display()))
                .ok()
        });
    let whole_page = page.is_some() && a.kind == "page";
    if let Some(pix) = &page {
        let max_h = if whole_page {
            bottom - 56.0 - y
        } else {
            0.42 * hf
        };
        let scale = (inner / f64::from(pix.width())).min(max_h / f64::from(pix.height()));
        let (pw, ph) = (
            f64::from(pix.width()) * scale,
            f64::from(pix.height()) * scale,
        );
        cr.save().map_err(|e| e.to_string())?;
        rounded(&cr, MARGIN, y, pw, ph, 14.0);
        cr.clip();
        cr.translate(MARGIN, y);
        cr.scale(scale, scale);
        cr.set_source_surface(pix, 0.0, 0.0)
            .map_err(|e| e.to_string())?;
        cr.paint().map_err(|e| e.to_string())?;
        cr.restore().map_err(|e| e.to_string())?;
        y += ph + 48.0;
    }

    if !whole_page {
        // The quote and note, as large as fits: short quotes can be large,
        // long ones shrink.
        let room = bottom - 84.0 - y;
        let mut size = 88.0;
        let (quote_l, note_l) = loop {
            let q = layout(&ctx, &text, SERIF, size, inner);
            let n = note.as_deref().map(|n| {
                layout(
                    &ctx,
                    n,
                    &format!("{SERIF} Italic"),
                    (size * 0.62).max(22.0),
                    inner,
                )
            });
            let total = height(&q) + n.as_ref().map_or(0.0, |n| 36.0 + height(n));
            if total <= room || size <= 24.0 {
                if total > room {
                    // Too long even small: cut off with an ellipsis.
                    q.set_height((room.max(1.0) * f64::from(pango::SCALE)) as i32);
                    q.set_ellipsize(pango::EllipsizeMode::End);
                }
                break (q, n);
            }
            size -= 2.0;
        };
        let total = height(&quote_l) + note_l.as_ref().map_or(0.0, |n| 36.0 + height(n));
        // Short quotes sit in the middle of the space; with a page, under it.
        let mut ty = if page.is_some() {
            y
        } else {
            y + ((room - total) / 2.0).max(0.0)
        };
        // Whatever happens, the text stays clear of the foot.
        cr.save().map_err(|e| e.to_string())?;
        cr.rectangle(0.0, y - 20.0, w, bottom - 36.0 - (y - 20.0));
        cr.clip();
        set(&cr, fg);
        cr.move_to(MARGIN, ty);
        pangocairo::functions::show_layout(&cr, &quote_l);
        ty += height(&quote_l);
        if let Some(n) = &note_l {
            set(&cr, muted);
            cr.move_to(MARGIN, ty + 36.0);
            pangocairo::functions::show_layout(&cr, n);
        }
        cr.restore().map_err(|e| e.to_string())?;
    }

    set(&cr, fg);
    cr.move_to(MARGIN, bottom);
    pangocairo::functions::show_layout(&cr, &title);
    if let Some(au) = &author {
        set(&cr, muted);
        cr.move_to(MARGIN, bottom + height(&title) + 6.0);
        pangocairo::functions::show_layout(&cr, au);
    }
    if o.credit {
        let k = layout(&ctx, "Kollate", "Cantarell Bold", 22.0, inner);
        k.set_alignment(pango::Alignment::Right);
        set(&cr, muted);
        cr.move_to(MARGIN, hf - MARGIN + 30.0 - height(&k));
        pangocairo::functions::show_layout(&cr, &k);
    }

    drop(cr);
    let mut png = Vec::new();
    surface.write_to_png(&mut png).map_err(|e| e.to_string())?;
    Ok(png)
}

/// A page image as a Cairo surface, decoded in this process.
fn page_surface(path: &Path) -> Result<cairo::ImageSurface, String> {
    let rgb = image::open(path).map_err(|e| e.to_string())?.to_rgb8();
    let (w, h) = (rgb.width() as i32, rgb.height() as i32);
    let stride = cairo::Format::Rgb24
        .stride_for_width(rgb.width())
        .map_err(|e| e.to_string())?;
    let mut data = vec![0u8; stride as usize * h as usize];
    for (y, row) in rgb.rows().enumerate() {
        for (x, p) in row.enumerate() {
            // Cairo's RGB24 is a native-endian u32 of 0xXXRRGGBB.
            let i = y * stride as usize + x * 4;
            data[i..i + 4]
                .copy_from_slice(&u32::from_be_bytes([0, p[0], p[1], p[2]]).to_ne_bytes());
        }
    }
    cairo::ImageSurface::create_for_data(data, cairo::Format::Rgb24, w, h, stride)
        .map_err(|e| e.to_string())
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// A file name for a card of `a`: "Moby-Dick – quote.png".
pub fn file_name(a: &Annotation) -> String {
    let title: String = a
        .book_title
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    format!(
        "{} – quote.png",
        title.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// The email draft's subject and body for a card of `a`.
pub fn email_text(a: &Annotation, o: CardOptions) -> (String, String) {
    let (text, note) = quote(a).unwrap_or_default();
    let mut body = format!("“{text}”\n\n— {}", a.book_title);
    if let Some(author) = a.book_author.as_deref().filter(|_| a.kind != "page") {
        body.push_str(&format!(", {author}"));
    }
    if let Some(note) = note.filter(|_| o.note) {
        body.push_str(&format!("\n\n{note}"));
    }
    (format!("A quote from {}", a.book_title), body)
}

/// Opens a draft in the user's email app (Thunderbird, Evolution…) through
/// the desktop's Email portal, with the card attached. Kollate itself sends
/// nothing; the email app does, if the user presses Send.
/// `to` fills in the recipient (e.g. the user's own address).
pub async fn compose_email(
    to: Option<&str>,
    subject: &str,
    body: &str,
    attachment: &Path,
) -> Result<(), glib::Error> {
    let file = std::fs::File::open(attachment)
        .map_err(|e| glib::Error::new(gio::IOErrorEnum::NotFound, &e.to_string()))?;
    let fds = gio::UnixFDList::new();
    let index = fds.append(&file)?;
    let options = glib::VariantDict::new(None);
    if let Some(to) = to.map(str::trim).filter(|t| !t.is_empty()) {
        options.insert_value("address", &to.to_variant());
    }
    options.insert_value("subject", &subject.to_variant());
    options.insert_value("body", &body.to_variant());
    options.insert_value(
        "attachment_fds",
        &glib::Variant::array_from_iter_with_type(
            glib::VariantTy::HANDLE,
            [glib::variant::Handle(index).to_variant()],
        ),
    );
    let params = glib::Variant::tuple_from_iter(["".to_variant(), options.end()]);
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    bus.call_with_unix_fd_list_future(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Email",
        "ComposeEmail",
        Some(&params),
        Some(glib::VariantTy::new("(o)").expect("valid type")),
        gio::DBusCallFlags::NONE,
        -1,
        Some(&fds),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders cards for every annotation in `$KOLLATE_CARDS_LIBRARY` into
    /// `$KOLLATE_CARDS_OUT`, to look at (not run without them).
    #[test]
    fn renders_cards() {
        let (Some(lib), Some(out)) = (
            std::env::var_os("KOLLATE_CARDS_LIBRARY"),
            std::env::var_os("KOLLATE_CARDS_OUT"),
        ) else {
            return;
        };
        let lib = kollate_core::Library::open(Path::new(&lib)).unwrap();
        let all = lib
            .query_annotations(&kollate_core::store::AnnotationFilter {
                view: kollate_core::store::View::All,
                ..Default::default()
            })
            .unwrap();
        for a in all {
            for (tag, o) in [
                (
                    "sq-light",
                    CardOptions {
                        tall: false,
                        dark: false,
                        note: true,
                        page: true,
                        credit: false,
                    },
                ),
                (
                    "tall-dark",
                    CardOptions {
                        tall: true,
                        dark: true,
                        note: true,
                        page: true,
                        credit: true,
                    },
                ),
            ] {
                if let Ok(png) = render(&a, o) {
                    std::fs::write(Path::new(&out).join(format!("{}-{tag}.png", a.id)), png)
                        .unwrap();
                }
            }
        }
    }
}
