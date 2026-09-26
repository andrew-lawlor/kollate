//! The Preferences dialog, including dictionaries.

use super::*;

impl Window {
    pub(super) fn show_preferences(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::new();
        self.track_dialog(&dialog);
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .title("Kobo")
            .description("Kollate only ever reads from your Kobo. Nothing on it is changed.")
            .build();
        let on_connect = adw::ComboRow::builder()
            .title("When a Kobo Is Connected")
            .model(&gtk::StringList::new(&OnConnect::LABELS))
            .build();
        let current = OnConnect::ALL
            .iter()
            .position(|v| *v == self.on_connect_setting())
            .unwrap_or(0);
        on_connect.set_selected(current as u32);
        let weak = Rc::downgrade(self);
        on_connect.connect_selected_notify(move |row| {
            let Some(this) = weak.upgrade() else { return };
            let value = OnConnect::ALL[(row.selected() as usize).min(OnConnect::ALL.len() - 1)];
            if let Err(err) = this.lib.borrow().set_setting("on_connect", value.key()) {
                this.error("Couldn’t Save Preference", err);
            }
        });
        group.add(&on_connect);

        let delete_policy = adw::ComboRow::builder()
            .title("When a Highlight Is Deleted on the Kobo")
            .model(&gtk::StringList::new(&[
                "Keep it, marked as deleted",
                "Move it to Trash",
            ]))
            .build();
        let policies = [DeviceDeletePolicy::Keep, DeviceDeletePolicy::Trash];
        let current = self.lib.borrow().device_delete_policy().unwrap_or_default();
        delete_policy.set_selected(policies.iter().position(|p| *p == current).unwrap_or(0) as u32);
        delete_policy
            .set_subtitle("Highlights moved to Trash go back if they reappear on the Kobo.");
        let weak = Rc::downgrade(self);
        delete_policy.connect_selected_notify(move |row| {
            let Some(this) = weak.upgrade() else { return };
            let policy = policies[(row.selected() as usize).min(1)];
            if let Err(err) = this.lib.borrow().set_device_delete_policy(policy) {
                this.error("Couldn’t Save Preference", err);
            }
        });
        group.add(&delete_policy);

        page.add(&group);
        for group in self.dictionaries_groups(&dialog) {
            page.add(&group);
        }

        if let Some(dir) = self.lib.borrow().assets_dir() {
            let location = adw::ActionRow::builder()
                .title("Library Location")
                .subtitle(dir.display().to_string())
                .subtitle_selectable(true)
                .build();
            let library = adw::PreferencesGroup::builder().title("Library").build();
            library.add(&location);
            page.add(&library);
        }
        dialog.add(&page);
        dialog.present(Some(&self.win));
    }

    /// The Dictionaries section: a header group (explanation, add button,
    /// where to get more) and one group per language, in lookup order.
    fn dictionaries_groups(
        self: &Rc<Self>,
        dialog: &adw::PreferencesDialog,
    ) -> Vec<adw::PreferencesGroup> {
        let header = adw::PreferencesGroup::builder()
            .title("Dictionaries")
            .description(
                "Each word is looked up in the dictionaries for its language. Dictionaries you add are tried \
                 before the included one. Everything stays on this computer.",
            )
            .build();
        let user_dir = self.lib.borrow().assets_dir().map(|d| dict::user_dir(&d));
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add a dictionary (DictFile .df.bz2, StarDict .ifo or kaikki.org .jsonl)")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .sensitive(user_dir.is_some())
            .build();
        add.update_property(&[gtk::accessible::Property::Label("Add Dictionary")]);
        header.set_header_suffix(Some(&add));
        let weak = Rc::downgrade(self);
        add.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.add_dictionary(&dialog);
                }
            }
        ));

        // Free Wiktionary dictionaries for other languages, with credit.
        let more = adw::ActionRow::builder()
            .title("Dictionaries for Other Languages")
            .subtitle("Free Wiktionary dictionaries from reader.dict. Download the DictFile version (.df.bz2), then add it with +.")
            .activatable(true)
            .build();
        more.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        let weak = Rc::downgrade(self);
        more.connect_activated(move |_| {
            if let Some(this) = weak.upgrade() {
                gtk::UriLauncher::new(MORE_DICTIONARIES_URL).launch(
                    Some(&this.win),
                    gio::Cancellable::NONE,
                    |_| {},
                );
            }
        });

        let dicts = self.dictionaries.borrow();
        if dicts.is_empty() {
            header.add(
                &adw::ActionRow::builder()
                    .title("No Dictionaries")
                    .subtitle("Words can still be defined by hand.")
                    .build(),
            );
        }
        header.add(&more);
        let mut groups = vec![header];

        // One group per language, languages by name, dictionaries in lookup order.
        let mut languages: Vec<Option<&str>> = Vec::new();
        for d in dicts.iter() {
            if !languages.contains(&d.language.as_deref()) {
                languages.push(d.language.as_deref());
            }
        }
        languages.sort_by_key(|l| (l.is_none(), language_name(*l)));
        for language in languages {
            let group = adw::PreferencesGroup::builder()
                .title(language_name(language))
                .build();
            for d in dicts.iter().filter(|d| d.language.as_deref() == language) {
                let imported = user_dir.as_ref().is_some_and(|u| d.path.starts_with(u));
                let mut parts = vec![if imported {
                    "Added by you".to_owned()
                } else {
                    "Included".to_owned()
                }];
                if let Some(source) = &d.source {
                    parts.push(source.clone());
                }
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&d.name))
                    .subtitle(glib::markup_escape_text(&parts.join(" · ")))
                    .build();
                if imported {
                    let remove = gtk::Button::builder()
                        .icon_name("user-trash-symbolic")
                        .tooltip_text("Remove")
                        .valign(gtk::Align::Center)
                        .css_classes(["flat"])
                        .build();
                    remove
                        .update_property(&[gtk::accessible::Property::Label("Remove Dictionary")]);
                    let path = d.path.clone();
                    let weak = Rc::downgrade(self);
                    remove.connect_clicked(glib::clone!(
                        #[weak]
                        dialog,
                        move |_| {
                            let Some(this) = weak.upgrade() else { return };
                            this.dictionaries.borrow_mut().clear();
                            if let Err(err) = std::fs::remove_file(&path) {
                                this.error("Couldn’t Remove Dictionary", err);
                            }
                            this.load_dictionaries();
                            dialog.close();
                            this.show_preferences();
                        }
                    ));
                    row.add_suffix(&remove);
                }
                group.add(&row);
            }
            groups.push(group);
        }
        groups
    }

    /// Converts a user-chosen StarDict or kaikki.org file into the user's
    /// dictionary folder, then defines any words still missing a definition.
    fn add_dictionary(self: &Rc<Self>, prefs: &adw::PreferencesDialog) {
        let Some(user_dir) = self.lib.borrow().assets_dir().map(|d| dict::user_dir(&d)) else {
            return;
        };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Dictionaries"));
        for pattern in [
            "*.df",
            "*.df.bz2",
            "*.df.gz",
            "*.ifo",
            "*.jsonl",
            "*.jsonl.gz",
            "*.json",
            "*.json.gz",
        ] {
            filter.add_pattern(pattern);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title("Add Dictionary")
            .filters(&filters)
            .modal(true)
            .build();
        let this = self.clone();
        let prefs = prefs.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = chooser.open_future(Some(&this.win)).await else {
                return;
            };
            let Some(input) = file.path() else { return };
            let name = input
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let stem = name.split('.').next().unwrap_or("dictionary").to_owned();
            let output = user_dir.join(format!("{stem}.db"));
            this.show_toast(
                adw::Toast::builder()
                    .title(format!("Adding {name}…"))
                    .timeout(0)
                    .build(),
            );
            let built = gio::spawn_blocking(move || -> kollate_core::Result<usize> {
                std::fs::create_dir_all(output.parent().expect("has parent"))?;
                if name.ends_with(".ifo") {
                    dict::build_from_stardict(&input, &output)
                } else if name.contains(".df") {
                    // reader.dict names files dict-<lang>-<lang>.df.bz2.
                    let lang = stem
                        .strip_prefix("dict-")
                        .and_then(|r| r.split('-').next())
                        .filter(|l| l.len() <= 3);
                    let title = match lang {
                        Some(l) => format!("{} Wiktionary", language_name(Some(l))),
                        None => format!("Wiktionary ({stem})"),
                    };
                    dict::build_from_dictfile(&input, &output, &title, lang, WIKTIONARY_SOURCE)
                } else {
                    dict::build_from_kaikki(&input, &output, &format!("Wiktionary ({stem})"))
                }
            })
            .await;
            match built {
                Ok(Ok(senses)) => {
                    this.load_dictionaries();
                    let defined = this.enrich_vocab();
                    this.reload();
                    // Reopen Preferences first, so the toast lands on the new dialog.
                    prefs.close();
                    this.show_preferences();
                    this.toast(&format!(
                        "Added {} entries · {}",
                        senses,
                        plural(defined, "word defined", "words defined")
                    ));
                }
                Ok(Err(err)) => this.error("Couldn’t Add Dictionary", err),
                Err(_) => this.error(
                    "Couldn’t Add Dictionary",
                    "The conversion stopped unexpectedly.",
                ),
            }
        });
    }
}

/// English name of an ISO 639-1 language code, for Preferences.
fn language_name(code: Option<&str>) -> String {
    let Some(code) = code else {
        return "Any Language".to_owned();
    };
    let name = match code {
        "ar" => "Arabic",
        "ca" => "Catalan",
        "cs" => "Czech",
        "da" => "Danish",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "eo" => "Esperanto",
        "es" => "Spanish",
        "fi" => "Finnish",
        "fr" => "French",
        "he" => "Hebrew",
        "hu" => "Hungarian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "la" => "Latin",
        "nb" | "no" => "Norwegian",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sv" => "Swedish",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "zh" => "Chinese",
        other => return other.to_uppercase(),
    };
    name.to_owned()
}

#[cfg(test)]
mod language_tests {
    use super::language_name;

    #[test]
    fn names_languages() {
        assert_eq!(language_name(Some("de")), "German");
        assert_eq!(language_name(Some("en")), "English");
        assert_eq!(language_name(Some("xq")), "XQ");
        assert_eq!(language_name(None), "Any Language");
    }
}
