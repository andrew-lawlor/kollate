//! The Export dialog and Obsidian sync.

use super::*;

impl Window {
    fn flag(&self, key: &str) -> bool {
        self.lib.borrow().setting(key).ok().flatten().as_deref() == Some("1")
    }

    fn set_flag(&self, key: &str, on: bool) {
        if let Err(err) = self
            .lib
            .borrow()
            .set_setting(key, if on { "1" } else { "0" })
        {
            self.error("Couldn’t Save Setting", err);
        }
    }

    fn export_options(&self) -> ExportOptions {
        ExportOptions {
            include_archived: self.flag("export_archived"),
            include_known_words: self.flag("anki_known"),
            everything: false,
        }
    }

    fn obsidian_folder(&self) -> Option<PathBuf> {
        self.lib
            .borrow()
            .setting("obsidian_folder")
            .ok()
            .flatten()
            .map(PathBuf::from)
    }

    fn sync_obsidian_now(&self) -> Option<String> {
        let folder = self.obsidian_folder()?;
        if let Err(err) = not_on_kobo(&folder) {
            self.error("Couldn’t Sync to Obsidian", err);
            return None;
        }
        let result = export::sync_obsidian(&self.lib.borrow(), &folder, self.export_options());
        match result {
            Ok(s) if s.notes_written == 0 => Some("Obsidian notes are up to date".to_owned()),
            Ok(s) => Some(format!(
                "Updated {} in Obsidian",
                plural(s.notes_written, "note", "notes")
            )),
            Err(err) => {
                self.error("Couldn’t Sync to Obsidian", err);
                None
            }
        }
    }

    pub(super) fn auto_sync_obsidian(&self) {
        if self.flag("obsidian_auto")
            && let Some(message) = self.sync_obsidian_now()
        {
            // Shown after the import summary rather than replacing it.
            let toast = adw::Toast::builder().title(message).timeout(4).build();
            self.toasts.add_toast(toast);
        }
    }

    /// Toast for a finished export with a button to show the file.
    fn exported(&self, message: String, path: PathBuf) {
        let toast = adw::Toast::builder()
            .title(message)
            .button_label("Show")
            .timeout(6)
            .build();
        let win = self.win.clone();
        toast.connect_button_clicked(move |_| {
            gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).open_containing_folder(
                Some(&win),
                gio::Cancellable::NONE,
                |_| {},
            );
        });
        self.show_toast(toast);
    }

    /// Asks where to save, then runs `write` and reports its result.
    fn export_file(
        self: &Rc<Self>,
        title: &str,
        default_name: &str,
        write: impl Fn(&Library, &std::path::Path, ExportOptions) -> kollate_core::Result<String>
        + 'static,
    ) {
        let dialog = gtk::FileDialog::builder()
            .title(title)
            .initial_name(default_name)
            .modal(true)
            .build();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.save_future(Some(&this.win)).await else {
                return;
            };
            let Some(path) = file.path() else { return };
            if let Err(err) = not_on_kobo(&path) {
                return this.error("Export Failed", err);
            }
            let result = write(&this.lib.borrow(), &path, this.export_options());
            match result {
                Ok(message) => this.exported(message, path),
                Err(err) => this.error("Export Failed", err),
            }
        });
    }

    pub(super) fn show_export(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::builder().title("Export").build();
        self.track_dialog(&dialog);
        let page = adw::PreferencesPage::new();
        let button = |label: &str, suggested: bool| {
            let b = gtk::Button::builder()
                .label(label)
                .valign(gtk::Align::Center)
                .build();
            if suggested {
                b.add_css_class("suggested-action");
            }
            b
        };
        let switch = |title: &str, subtitle: Option<&str>, key: &'static str| {
            let row = adw::SwitchRow::builder()
                .title(title)
                .active(self.flag(key))
                .build();
            if let Some(sub) = subtitle {
                row.set_subtitle(sub);
            }
            let weak = Rc::downgrade(self);
            row.connect_active_notify(move |r| {
                if let Some(this) = weak.upgrade() {
                    this.set_flag(key, r.is_active());
                }
            });
            row
        };

        // Obsidian
        let obsidian = adw::PreferencesGroup::builder()
            .title("Obsidian")
            .description(
                "Kollate writes one note per book, plus a Vocabulary note, and rewrites them each time it syncs. \
                 To add your own thoughts about a book, write at the bottom of its note, below the line that \
                 starts with “kollate:user”; Kollate never changes that part. For a thought about one highlight, \
                 add a note to it here in Kollate (E) and it appears under the highlight.",
            )
            .build();
        let folder_row = adw::ActionRow::builder()
            .title("Folder in Your Vault")
            .subtitle(
                self.obsidian_folder()
                    .map_or("Not chosen".to_owned(), |p| portal::display_path(&p)),
            )
            .build();
        let choose = button("Choose…", false);
        folder_row.add_suffix(&choose);
        obsidian.add(&folder_row);
        obsidian.add(&switch("Sync After Every Import", None, "obsidian_auto"));
        let sync_row = adw::ActionRow::builder().title("Sync Now").build();
        let sync = button("Sync", true);
        sync.set_sensitive(self.obsidian_folder().is_some());
        sync_row.add_suffix(&sync);
        obsidian.add(&sync_row);
        let weak = Rc::downgrade(self);
        sync.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade()
                && let Some(message) = this.sync_obsidian_now()
                && let Some(folder) = this.obsidian_folder()
            {
                this.exported(message, folder.join("Vocabulary.md"));
            }
        });
        let weak = Rc::downgrade(self);
        choose.connect_clicked(glib::clone!(
            #[weak]
            folder_row,
            #[weak]
            sync,
            move |_| {
                let Some(this) = weak.upgrade() else { return };
                let chooser = gtk::FileDialog::builder()
                    .title("Choose a Folder in Your Obsidian Vault")
                    .modal(true)
                    .build();
                if let Some(current) = this.obsidian_folder() {
                    chooser.set_initial_folder(Some(&gio::File::for_path(current)));
                }
                glib::spawn_future_local(async move {
                    let Ok(folder) = chooser.select_folder_future(Some(&this.win)).await else {
                        return;
                    };
                    let Some(path) = folder.path() else { return };
                    if let Err(err) = not_on_kobo(&path) {
                        return this.error("That Folder Is on Your Kobo", err);
                    }
                    if let Err(err) = this
                        .lib
                        .borrow()
                        .set_setting("obsidian_folder", &path.to_string_lossy())
                    {
                        return this.error("Couldn’t Save Setting", err);
                    }
                    folder_row.set_subtitle(&portal::display_path(&path));
                    sync.set_sensitive(true);
                });
            }
        ));
        page.add(&obsidian);

        // Anki
        let anki = adw::PreferencesGroup::builder()
            .title("Anki")
            .description("Vocabulary cards (word → meaning, and fill in the blank). Import the file in Anki; exporting again updates your cards and keeps their progress.")
            .build();
        anki.add(&switch(
            "Include a Highlights Deck",
            None,
            "anki_highlights",
        ));
        anki.add(&switch("Include Words Marked Known", None, "anki_known"));
        let anki_row = adw::ActionRow::builder()
            .title("Anki Package")
            .subtitle(".apkg")
            .build();
        let anki_button = button("Export…", true);
        anki_row.add_suffix(&anki_button);
        anki.add(&anki_row);
        let weak = Rc::downgrade(self);
        anki_button.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let highlights = this.flag("anki_highlights");
            this.export_file(
                "Export Anki Deck",
                "Kollate.apkg",
                move |lib, path, opts| {
                    let s = export::export_anki(lib, path, opts, highlights)?;
                    Ok(format!(
                        "Exported {} for Anki",
                        plural(s.cards, "card", "cards")
                    ))
                },
            );
        });
        page.add(&anki);

        // Files
        let files = adw::PreferencesGroup::builder().title("Files").build();
        type Writer = fn(&Library, &std::path::Path, ExportOptions) -> kollate_core::Result<String>;
        let formats: [(&str, &str, &str, Writer); 4] = [
            (
                "Backup",
                "JSON with everything, including trashed items",
                "Kollate backup.json",
                |lib, path, _| {
                    Ok(format!(
                        "Backed up {}",
                        plural(export::export_json(lib, path)?, "book", "books")
                    ))
                },
            ),
            (
                "Highlights",
                "CSV",
                "Kollate highlights.csv",
                |lib, path, opts| {
                    Ok(format!(
                        "Exported {}",
                        plural(
                            export::export_highlights_csv(lib, path, opts)?,
                            "highlight",
                            "highlights"
                        )
                    ))
                },
            ),
            (
                "Vocabulary",
                "CSV",
                "Kollate vocabulary.csv",
                |lib, path, opts| {
                    Ok(format!(
                        "Exported {}",
                        plural(export::export_vocab_csv(lib, path, opts)?, "word", "words")
                    ))
                },
            ),
            (
                "Readwise",
                "CSV in Readwise’s import format",
                "Kollate for Readwise.csv",
                |lib, path, opts| {
                    Ok(format!(
                        "Exported {}",
                        plural(
                            export::export_readwise_csv(lib, path, opts)?,
                            "highlight",
                            "highlights"
                        )
                    ))
                },
            ),
        ];
        for (title, subtitle, name, write) in formats {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(subtitle)
                .build();
            let b = button("Export…", false);
            row.add_suffix(&b);
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.export_file(&format!("Export {title}"), name, write);
                }
            });
            files.add(&row);
        }
        page.add(&files);

        let options = adw::PreferencesGroup::builder().title("Options").build();
        options.add(&switch(
            "Include Archived Highlights",
            Some("Trashed highlights are never exported (except in the backup)."),
            "export_archived",
        ));
        page.add(&options);

        dialog.add(&page);
        dialog.present(Some(&self.win));
    }
}

/// Refuses destinations on a Kobo. In the Flatpak a chosen location arrives
/// as a document-portal path, so its real location is checked as well (a
/// subfolder of the Kobo wouldn't otherwise be recognisable).
fn not_on_kobo(path: &std::path::Path) -> kollate_core::Result<()> {
    kollate_core::kobo::ensure_not_on_kobo(path)?;
    match portal::host_path(path) {
        Some(real) => kollate_core::kobo::ensure_not_on_kobo(&real),
        None => Ok(()),
    }
}
