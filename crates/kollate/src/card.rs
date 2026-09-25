//! The widget showing one annotation.

use adw::prelude::*;
use chrono::Local;
use gtk::gio;
use kollate_core::kobo::color_name;
use kollate_core::store::{Annotation, Book, Status};

pub fn format_date(date: chrono::DateTime<chrono::Utc>) -> String {
    date.with_timezone(&Local).format("%-d %b %Y").to_string()
}

fn pill(text: &str, warning: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("pill");
    if warning {
        label.add_css_class("warning");
    }
    label
}

fn wrapped_label(text: &str, css: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(css.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .build();
    label.set_natural_wrap_mode(gtk::NaturalWrapMode::Word);
    label
}

/// The card's popover menu; items are actions in the row's `card` group.
fn menu(a: &Annotation) -> gio::Menu {
    let menu = gio::Menu::new();
    let main = gio::Menu::new();
    main.append(Some("_Edit…"), Some("card.edit"));
    main.append(Some("_Copy Text"), Some("card.copy"));
    main.append(Some("Copy as _Markdown"), Some("card.copy-markdown"));
    if a.markup_image.is_some() {
        main.append(Some("_Open Page Image"), Some("card.open-image"));
    }
    menu.append_section(None, &main);

    let status = gio::Menu::new();
    match a.status {
        Status::Inbox => {
            status.append(Some("_Keep"), Some("card.keep"));
            status.append(Some("_Archive"), Some("card.archive"));
        }
        Status::Kept => {
            status.append(Some("Move to _Inbox"), Some("card.inbox"));
            status.append(Some("_Archive"), Some("card.archive"));
        }
        Status::Archived | Status::Trashed => {
            status.append(Some("_Restore"), Some("card.keep"));
        }
    }
    if a.status != Status::Trashed {
        status.append(Some("Move to _Trash"), Some("card.trash"));
    }
    menu.append_section(None, &status);

    if a.device_changed_at.is_some() {
        let device = gio::Menu::new();
        device.append(Some("Use _Kobo’s Version"), Some("card.accept-device"));
        menu.append_section(None, &device);
    }
    menu
}

/// Accessible name for an icon-only control: the tooltip minus its shortcut hint.
fn set_accessible_label(widget: &impl IsA<gtk::Accessible>, tooltip: &str) {
    let label = tooltip.split(" (").next().unwrap_or(tooltip);
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}

fn icon_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .action_name(action)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    set_accessible_label(&button, tooltip);
    button
}

/// Builds the card. In a book view the list is grouped by chapter, so the
/// chapter is left out of the details line; elsewhere it's grouped by book.
pub fn build(a: &Annotation, in_book_view: bool) -> gtk::Widget {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(14)
        .margin_top(14)
        .margin_bottom(8)
        .margin_start(14)
        .margin_end(10)
        .build();

    let bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bar.add_css_class("color-bar");
    bar.add_css_class(&match color_name(a.color) {
        "unknown" => "hl-unknown".to_owned(),
        _ => format!("hl-{}", a.color),
    });
    bar.set_margin_bottom(6);
    bar.set_tooltip_text(Some(color_name(a.color)));
    root.append(&bar);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_hexpand(true);
    match (a.kind.as_str(), a.text()) {
        (_, Some(text)) => body.append(&wrapped_label(text, &["quote"])),
        ("markup", None) => match a.markup_image.as_ref().filter(|p| p.is_file()) {
            Some(path) => {
                let picture = gtk::Picture::builder()
                    .file(&gtk::gio::File::for_path(path))
                    .content_fit(gtk::ContentFit::Contain)
                    .can_shrink(true)
                    .height_request(320)
                    .halign(gtk::Align::Start)
                    .css_classes(["markup-image"])
                    .build();
                picture.set_tooltip_text(Some("Handwritten markup (page image from your Kobo)"));
                picture.update_property(&[gtk::accessible::Property::Label(
                    "Handwritten markup page image",
                )]);
                body.append(&picture);
            }
            None => {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.append(&gtk::Image::from_icon_name("input-tablet-symbolic"));
                row.append(&wrapped_label(
                    "Handwritten markup. The page image is copied when the Kobo is connected.",
                    &["dim-label"],
                ));
                body.append(&row);
            }
        },
        _ => body.append(&wrapped_label("(no text)", &["dim-label"])),
    }

    if let Some(note) = a.note() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::from_icon_name("document-edit-symbolic");
        icon.set_valign(gtk::Align::Start);
        icon.add_css_class("dim-label");
        row.append(&icon);
        row.append(&wrapped_label(note, &["note"]));
        body.append(&row);
    }

    // Details line: chapter · date, pills, then actions.
    let details = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let mut meta = Vec::new();
    if !in_book_view && let Some(chapter) = &a.chapter_title {
        meta.push(chapter.clone());
    }
    if let Some(date) = a.created_at {
        meta.push(format_date(date));
    }
    let meta_label = gtk::Label::builder()
        .label(meta.join(" · "))
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["caption", "dim-label"])
        .build();
    meta_label.set_tooltip_text(Some(&meta.join("\n")));
    details.append(&meta_label);
    for tag in &a.tags {
        details.append(&pill(tag, false));
    }
    if a.user_text.is_some() || a.user_note.is_some() {
        details.append(&pill("edited", false));
    }
    if a.device_changed_at.is_some() {
        let p = pill("changed on Kobo", true);
        p.set_tooltip_text(Some(
            "Edited on the Kobo after you edited it here. Your version is shown.",
        ));
        details.append(&p);
    }
    if a.removed_on_device_at.is_some() {
        let p = pill("deleted on Kobo", true);
        p.set_tooltip_text(Some("No longer on the Kobo. Kollate keeps it."));
        details.append(&p);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions.set_margin_start(6);
    let (star_icon, star_tip) = if a.starred {
        ("starred-symbolic", "Unstar (S)")
    } else {
        ("non-starred-symbolic", "Star (S)")
    };
    actions.append(&icon_button(star_icon, star_tip, "card.star"));
    if a.status == Status::Inbox {
        actions.append(&icon_button("checkmark-symbolic", "Keep (K)", "card.keep"));
        actions.append(&icon_button(
            "folder-symbolic",
            "Archive (A)",
            "card.archive",
        ));
    }
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("More")
        .menu_model(&menu(a))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    set_accessible_label(&more, "More");
    actions.append(&more);
    details.append(&actions);
    body.append(&details);
    root.append(&body);
    root.upcast()
}

/// The header row at the top of a book's page: cover, title and progress.
pub fn book_header(book: &Book) -> gtk::ListBoxRow {
    let hbox = gtk::Box::builder()
        .spacing(18)
        .margin_top(6)
        .margin_bottom(12)
        .build();
    let cover = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(96)
        .height_request(144)
        .valign(gtk::Align::Start)
        .css_classes(["book-cover"])
        .build();
    match book.cover.as_ref().filter(|p| p.is_file()) {
        Some(path) => cover.set_filename(Some(path)),
        None => cover.add_css_class("no-cover"),
    }
    cover.update_property(&[gtk::accessible::Property::Label("Cover")]);
    hbox.append(&cover);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .valign(gtk::Align::Center)
        .build();
    text.append(&wrapped_label(&book.title, &["title-2"]));
    if let Some(author) = &book.author {
        text.append(&wrapped_label(author, &["dim-label"]));
    }
    let mut facts = Vec::new();
    if let Some(p) = book.percent_read.filter(|p| *p > 0) {
        facts.push(format!("{p}% read"));
    }
    if let Some(date) = book.last_read_at {
        facts.push(format!("last read {}", format_date(date)));
    }
    if !facts.is_empty() {
        let label = wrapped_label(&facts.join(" · "), &["caption", "dim-label"]);
        label.set_margin_top(6);
        text.append(&label);
    }
    hbox.append(&text);

    gtk::ListBoxRow::builder()
        .child(&hbox)
        .selectable(false)
        .activatable(false)
        .focusable(false)
        .css_classes(["book-header"])
        .build()
}

/// Markdown blockquote with the note and source, for "Copy as Markdown".
pub fn to_markdown(a: &Annotation) -> String {
    let mut out = String::new();
    if let Some(text) = a.text() {
        for line in text.lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }
    let source = match (&a.book_author, &a.chapter_title) {
        (Some(author), Some(ch)) => format!("{author}, *{}*, {ch}", a.book_title),
        (Some(author), None) => format!("{author}, *{}*", a.book_title),
        (None, Some(ch)) => format!("*{}*, {ch}", a.book_title),
        (None, None) => format!("*{}*", a.book_title),
    };
    out.push_str(&format!("> — {source}\n"));
    if let Some(note) = a.note() {
        out.push('\n');
        out.push_str(note);
        out.push('\n');
    }
    out
}
