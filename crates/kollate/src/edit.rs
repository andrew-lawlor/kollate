//! Dialog for correcting a highlight's text, editing its note and tags.
//! The Kobo's original stays in the library and can be restored.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use kollate_core::store::Annotation;

/// What the user saved. `None` means "use the Kobo's version".
pub struct Edited {
    pub text: Option<String>,
    pub note: Option<String>,
    pub tags: Vec<String>,
}

fn text_view(text: &str) -> gtk::TextView {
    let view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(10)
        .bottom_margin(10)
        .left_margin(12)
        .right_margin(12)
        .css_classes(["card"])
        .build();
    view.buffer().set_text(text);
    view
}

fn buffer_text(view: &gtk::TextView) -> String {
    let buf = view.buffer();
    buf.text(&buf.start_iter(), &buf.end_iter(), false)
        .trim()
        .to_owned()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .build()
}

/// An override is only stored when it differs from the Kobo's value.
fn override_of(edited: String, device: Option<&str>) -> Option<String> {
    (edited != device.unwrap_or("")).then_some(edited)
}

pub fn present(parent: &impl IsA<gtk::Widget>, a: &Annotation, on_save: impl Fn(Edited) + 'static) {
    let is_markup = a.kind == "markup" && a.device_text.is_none();
    let dialog = adw::Dialog::builder()
        .title(if is_markup {
            "Edit Markup"
        } else {
            "Edit Highlight"
        })
        .content_width(600)
        .content_height(560)
        .build();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .build();

    let text = text_view(a.text().unwrap_or(""));
    text.set_height_request(120);
    if !is_markup {
        body.append(&heading("Highlight"));
        body.append(&text);
    }
    let note = text_view(a.note().unwrap_or(""));
    note.set_height_request(90);
    let note_heading = heading("Note");
    note_heading.set_margin_top(12);
    body.append(&note_heading);
    body.append(&note);

    let tags_list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .margin_top(18)
        .build();
    let tags = adw::EntryRow::builder()
        .title("Tags, separated by commas")
        .text(a.tags.join(", "))
        .build();
    tags_list.append(&tags);
    body.append(&tags_list);

    let device_text = a.device_text.clone();
    let device_note = a.device_note.clone();
    if a.user_text.is_some() || a.user_note.is_some() {
        let original = heading("On Your Kobo");
        original.set_margin_top(18);
        body.append(&original);
        for (label, value) in [("Highlight", &device_text), ("Note", &device_note)] {
            if let Some(v) = value {
                body.append(
                    &gtk::Label::builder()
                        .label(format!("{label}: {v}"))
                        .wrap(true)
                        .xalign(0.0)
                        .selectable(true)
                        .css_classes(["dim-label"])
                        .build(),
                );
            }
        }
        let restore = gtk::Button::builder()
            .label("Restore Kobo Version")
            .halign(gtk::Align::Start)
            .css_classes(["pill"])
            .margin_top(6)
            .build();
        restore.connect_clicked(glib::clone!(
            #[weak]
            text,
            #[weak]
            note,
            #[strong]
            device_text,
            #[strong]
            device_note,
            move |_| {
                text.buffer().set_text(device_text.as_deref().unwrap_or(""));
                note.buffer().set_text(device_note.as_deref().unwrap_or(""));
            }
        ));
        body.append(&restore);
    }

    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    let cancel = gtk::Button::with_mnemonic("_Cancel");
    let save = gtk::Button::builder()
        .label("_Save")
        .use_underline(true)
        .css_classes(["suggested-action"])
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    dialog.set_default_widget(Some(&save));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .child(&body)
            .propagate_natural_height(true)
            .build(),
    ));
    dialog.set_child(Some(&toolbar));

    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    let on_save = Rc::new(on_save);
    save.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            on_save(Edited {
                text: if is_markup {
                    None
                } else {
                    override_of(buffer_text(&text), device_text.as_deref())
                },
                note: override_of(buffer_text(&note), device_note.as_deref()),
                tags: tags
                    .text()
                    .split(',')
                    .map(|t| t.trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .collect(),
            });
            dialog.close();
        }
    ));

    dialog.present(Some(parent));
}
