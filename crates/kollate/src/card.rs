//! The widget showing one annotation.

use adw::prelude::*;
use chrono::Local;
use gtk::gio;
use kollate_core::kobo::color_name;
use kollate_core::store::{Annotation, Status};

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
        ("markup", None) => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&gtk::Image::from_icon_name("input-tablet-symbolic"));
            row.append(&wrapped_label(
                "Handwritten markup. The page image is copied when the Kobo is connected.",
                &["dim-label"],
            ));
            body.append(&row);
        }
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
