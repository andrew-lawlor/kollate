//! Dialog for one vocabulary word: its definition and the sentence(s) it
//! was looked up in.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use kollate_core::kobo::epub::word_span;
use kollate_core::store::VocabDetail;

use crate::card::format_date;

/// What the user saved.
pub struct WordEdit {
    /// `Some` when the definition text was changed.
    pub definition: Option<String>,
    /// (sighting ID, chosen context) for sightings whose choice changed.
    pub contexts: Vec<(i64, Option<String>)>,
}

/// The context choice for one lookup: its sighting, the sentence chosen
/// before the dialog opened, and one radio button per candidate.
struct ContextChoice {
    sighting: i64,
    before: Option<String>,
    options: Vec<(gtk::CheckButton, String)>,
}

/// Pango markup for `sentence` with the first match of any of `words` in bold.
pub fn context_markup(sentence: &str, words: &[&str]) -> String {
    match words.iter().find_map(|w| word_span(sentence, w)) {
        Some((start, end)) => format!(
            "{}<b>{}</b>{}",
            glib::markup_escape_text(&sentence[..start]),
            glib::markup_escape_text(&sentence[start..end]),
            glib::markup_escape_text(&sentence[end..])
        ),
        None => glib::markup_escape_text(sentence).to_string(),
    }
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["heading"])
        .build()
}

/// Shows the dialog. `look_up_again` clears the definition and returns the
/// freshly looked-up one.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    detail: &VocabDetail,
    look_up_again: impl Fn() -> Option<String> + 'static,
    on_save: impl Fn(WordEdit) + 'static,
) {
    let v = &detail.vocab;
    let dialog = adw::Dialog::builder()
        .title(&v.word)
        .content_width(640)
        .content_height(620)
        .build();
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .build();

    // Definition
    let def_heading = heading("Definition");
    body.append(&def_heading);
    let definition = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(10)
        .bottom_margin(10)
        .left_margin(12)
        .right_margin(12)
        .height_request(100)
        .css_classes(["card"])
        .build();
    let original = v.definition.clone().unwrap_or_default();
    definition.buffer().set_text(&original);
    body.append(&definition);
    let source_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let source = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .css_classes(["caption", "dim-label"])
        .build();
    let describe = |src: Option<&str>| match src {
        Some("edited") => "Your definition".to_owned(),
        Some(name) => format!("From {name}"),
        None => {
            "Not found in your dictionaries. Add one in Preferences, or write your own.".to_owned()
        }
    };
    source.set_label(&describe(v.definition_source.as_deref()));
    source_row.append(&source);
    let again = gtk::Button::builder()
        .label("Look Up Again")
        .css_classes(["flat"])
        .build();
    again.set_tooltip_text(Some("Replace this definition with the dictionary’s"));
    source_row.append(&again);
    body.append(&source_row);
    again.connect_clicked(glib::clone!(
        #[weak]
        definition,
        #[weak]
        source,
        move |_| {
            let fresh = look_up_again();
            definition.buffer().set_text(fresh.as_deref().unwrap_or(""));
            source.set_label(&if fresh.is_some() {
                "Looked up again".to_owned()
            } else {
                describe(None)
            });
        }
    ));

    // Contexts, one section per lookup
    let lemma = v.lemma.clone().unwrap_or_default();
    let mut choices: Vec<ContextChoice> = Vec::new();
    for s in &detail.sightings {
        let mut title = String::from("Context");
        if let Some(book) = &s.book_title {
            title = format!("In {book}");
        }
        if let Some(date) = s.looked_up_at {
            title.push_str(&format!(" · {}", format_date(date)));
        }
        let h = heading(&title);
        h.set_margin_top(18);
        body.append(&h);
        if s.surface_form != v.word {
            body.append(
                &gtk::Label::builder()
                    .label(format!("Looked up as “{}”", s.surface_form))
                    .xalign(0.0)
                    .css_classes(["caption", "dim-label"])
                    .build(),
            );
        }

        let mut sentences = s.candidates.clone();
        if let Some(current) = &s.context
            && !sentences.contains(current)
        {
            sentences.insert(0, current.clone());
        }
        if sentences.is_empty() {
            body.append(
                &gtk::Label::builder()
                    .label("No sentences found yet. They’re read from the book when your Kobo is connected (not possible for store books with DRM).")
                    .wrap(true)
                    .xalign(0.0)
                    .css_classes(["dim-label"])
                    .build(),
            );
            continue;
        }
        let list = gtk::ListBox::builder()
            .css_classes(["boxed-list"])
            .selection_mode(gtk::SelectionMode::None)
            .build();
        let mut buttons = Vec::new();
        let mut group: Option<gtk::CheckButton> = None;
        let words = [s.surface_form.as_str(), v.word.as_str(), lemma.as_str()];
        for sentence in sentences {
            let label = gtk::Label::builder()
                .use_markup(true)
                .label(context_markup(&sentence, &words))
                .wrap(true)
                .xalign(0.0)
                .hexpand(true)
                .margin_start(6)
                .css_classes(["quote"])
                .build();
            let check = gtk::CheckButton::builder().child(&label).build();
            check.set_group(group.as_ref());
            group.get_or_insert_with(|| check.clone());
            check.set_active(s.context.as_deref() == Some(sentence.as_str()));
            let row = gtk::ListBoxRow::builder()
                .activatable(false)
                .child(&check)
                .build();
            check.set_margin_top(10);
            check.set_margin_bottom(10);
            check.set_margin_start(12);
            check.set_margin_end(12);
            list.append(&row);
            buttons.push((check, sentence));
        }
        body.append(&list);
        choices.push(ContextChoice {
            sighting: s.id,
            before: s.context.clone(),
            options: buttons,
        });
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
            let buf = definition.buffer();
            let text = buf
                .text(&buf.start_iter(), &buf.end_iter(), false)
                .trim()
                .to_owned();
            let contexts = choices
                .iter()
                .filter_map(|c| {
                    let chosen = c
                        .options
                        .iter()
                        .find(|(b, _)| b.is_active())
                        .map(|(_, s)| s.clone());
                    (chosen != c.before).then_some((c.sighting, chosen))
                })
                .collect();
            on_save(WordEdit {
                definition: (text != original).then_some(text),
                contexts,
            });
            dialog.close();
        }
    ));
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolds_the_word_and_escapes_markup() {
        assert_eq!(
            context_markup("A <b> theophany & more", &["theophanies", "theophany"]),
            "A &lt;b&gt; <b>theophany</b> &amp; more"
        );
        assert_eq!(context_markup("nothing here", &["word"]), "nothing here");
    }
}
