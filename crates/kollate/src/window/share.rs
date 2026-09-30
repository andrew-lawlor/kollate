//! The Share as Image dialog: a quote card, previewed as the options change,
//! copied, saved, or attached to an email draft.

use super::*;
use crate::share::{self, CardOptions};

/// Remembered choices ("1" on). Note and page are on unless turned off.
const TALL: &str = "card_tall";
const DARK: &str = "card_dark";
const NO_NOTE: &str = "card_no_note";
const NO_PAGE: &str = "card_no_page";
const CREDIT: &str = "card_credit";
/// The address Email… fills in (Preferences → Sharing): usually the user's own.
pub(super) const EMAIL_TO: &str = "card_email_to";

impl Window {
    fn card_options(&self) -> CardOptions {
        CardOptions {
            tall: self.flag(TALL),
            dark: self.flag(DARK),
            note: !self.flag(NO_NOTE),
            page: !self.flag(NO_PAGE),
            credit: self.flag(CREDIT),
        }
    }

    pub(super) fn share_card(self: &Rc<Self>, id: i64) {
        let Some(a) = self.lib.borrow().annotation(id).ok().flatten() else {
            return;
        };
        if share::quote(&a).is_none() {
            return;
        }
        let dialog = adw::Dialog::builder()
            .title("Share as Image")
            .content_width(560)
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .build();

        let preview = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .height_request(360)
            .css_classes(["card"])
            .build();
        preview.update_property(&[gtk::accessible::Property::Label(
            "Preview of the quote card",
        )]);
        body.append(&preview);

        let options = adw::PreferencesGroup::new();
        let pair = |a: &str, b: &str, second: bool| {
            let first = gtk::ToggleButton::with_label(a);
            let other = gtk::ToggleButton::with_label(b);
            other.set_group(Some(&first));
            other.set_active(second);
            first.set_active(!second);
            let linked = gtk::Box::builder()
                .css_classes(["linked"])
                .valign(gtk::Align::Center)
                .build();
            linked.append(&first);
            linked.append(&other);
            (linked, other)
        };
        let o = self.card_options();
        let (shape_box, tall) = pair("Square", "Tall", o.tall);
        let shape = adw::ActionRow::builder().title("Shape").build();
        shape.add_suffix(&shape_box);
        options.add(&shape);
        let (style_box, dark) = pair("Light", "Dark", o.dark);
        let style = adw::ActionRow::builder().title("Style").build();
        style.add_suffix(&style_box);
        options.add(&style);
        let note = adw::SwitchRow::builder()
            .title("Include Your Note")
            .active(o.note)
            .visible(a.text().is_some() && a.note().is_some())
            .build();
        options.add(&note);
        let page = adw::SwitchRow::builder()
            .title("Show the Page")
            .subtitle("With your ink on it")
            .active(o.page)
            .visible(matches!(a.kind.as_str(), "markup" | "page") && a.markup_view().is_some())
            .build();
        options.add(&page);
        let credit = adw::SwitchRow::builder()
            .title("Credit Kollate")
            .active(o.credit)
            .build();
        options.add(&credit);
        body.append(&options);

        // Always in view, under the options.
        let buttons = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::End)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(18)
            .margin_end(18)
            .build();
        let copy = gtk::Button::with_mnemonic("_Copy");
        let email = gtk::Button::with_mnemonic("_Email…");
        email.set_tooltip_text(Some(
            "Open a draft in your email app, with the card attached",
        ));
        let save = gtk::Button::builder()
            .label("_Save…")
            .use_underline(true)
            .css_classes(["suggested-action"])
            .build();
        buttons.append(&copy);
        buttons.append(&email);
        buttons.append(&save);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.add_bottom_bar(&buttons);
        toolbar.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .child(&body)
                .propagate_natural_height(true)
                .build(),
        ));
        dialog.set_child(Some(&toolbar));

        // The card as last drawn, redrawn whenever an option changes.
        let png: Rc<RefCell<Vec<u8>>> = Rc::default();
        let a = Rc::new(a);
        let redraw = Rc::new(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[weak]
            preview,
            #[weak]
            tall,
            #[weak]
            dark,
            #[weak]
            note,
            #[weak]
            page,
            #[weak]
            credit,
            #[strong]
            png,
            #[strong]
            a,
            move || {
                this.set_flag(TALL, tall.is_active());
                this.set_flag(DARK, dark.is_active());
                this.set_flag(NO_NOTE, !note.is_active());
                this.set_flag(NO_PAGE, !page.is_active());
                this.set_flag(CREDIT, credit.is_active());
                match share::render(&a, this.card_options()) {
                    Ok(bytes) => {
                        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes)) {
                            preview.set_paintable(Some(&texture));
                        }
                        png.replace(bytes);
                    }
                    Err(err) => eprintln!("kollate: couldn’t draw the card: {err}"),
                }
            }
        ));
        for b in [&tall, &dark] {
            let r = redraw.clone();
            b.connect_toggled(move |_| r());
        }
        for s in [&note, &page, &credit] {
            let r = redraw.clone();
            s.connect_active_notify(move |_| r());
        }
        redraw();

        copy.connect_clicked(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[strong]
            png,
            move |_| {
                if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(&*png.borrow())) {
                    this.win.clipboard().set_texture(&texture);
                    this.toast("Image copied");
                }
            }
        ));

        save.connect_clicked(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[weak]
            dialog,
            #[strong]
            png,
            #[strong]
            a,
            move |_| {
                let chooser = gtk::FileDialog::builder()
                    .title("Save Quote Card")
                    .initial_name(share::file_name(&a))
                    .modal(true)
                    .build();
                let (this, png) = (this.clone(), png.clone());
                glib::spawn_future_local(async move {
                    let Ok(file) = chooser.save_future(Some(&this.win)).await else {
                        return;
                    };
                    let Some(path) = file.path() else { return };
                    let written = exports::not_on_kobo(&path)
                        .and_then(|()| Ok(std::fs::write(&path, &*png.borrow())?));
                    match written {
                        Ok(()) => {
                            dialog.close();
                            this.toast("Image saved");
                        }
                        Err(err) => this.error("Couldn’t Save the Image", err),
                    }
                });
            }
        ));

        email.connect_clicked(glib::clone!(
            #[weak(rename_to = this)]
            self,
            #[weak]
            dialog,
            #[strong]
            png,
            #[strong]
            a,
            move |_| {
                // The email app gets the file itself, from Kollate's cache.
                let dir = glib::user_cache_dir().join("kollate").join("share");
                let path = dir.join(share::file_name(&a));
                if let Err(err) = std::fs::create_dir_all(&dir)
                    .and_then(|()| std::fs::write(&path, &*png.borrow()))
                {
                    return this.error("Couldn’t Prepare the Email", err);
                }
                let (subject, text) = share::email_text(&a, this.card_options());
                let address = this.lib.borrow().setting(EMAIL_TO).ok().flatten().unwrap_or_default();
                let this = this.clone();
                glib::spawn_future_local(async move {
                    let to = (!address.is_empty()).then_some(address.as_str());
                    match share::compose_email(to, &subject, &text, &path).await {
                        Ok(()) => {
                            dialog.close();
                        }
                        Err(err) => this.error(
                            "Couldn’t Open Your Email App",
                            format!(
                                "No email app answered ({err}). Save the image instead, and attach it \
                                 to an email yourself."
                            ),
                        ),
                    }
                });
            }
        ));

        dialog.present(Some(&self.win));
    }
}
