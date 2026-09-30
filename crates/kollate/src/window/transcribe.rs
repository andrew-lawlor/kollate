//! Reading stylus handwriting with a local model (SPEC §8a): the background
//! job, and the Handwriting group in Preferences.

use super::*;
use kollate_transcribe::{Installed, Role, Transcriber, catalog};

/// The model chosen in Preferences (a catalog ID).
const MODEL_KEY: &str = "transcribe_model";
/// Run on the CPU only (off by default: the GPU is used when there is one).
const CPU_ONLY: &str = "transcribe_cpu_only";

impl Window {
    fn models_dir(&self) -> Option<PathBuf> {
        self.lib.borrow().assets_dir().map(|d| d.join("models"))
    }

    fn chosen_model(&self) -> Option<Installed> {
        let installed = kollate_transcribe::installed(&self.models_dir()?);
        let preferred = self.lib.borrow().setting(MODEL_KEY).ok().flatten();
        kollate_transcribe::choose(&installed, preferred.as_deref())
    }

    /// Reads, in the background, every markup the chosen model hasn't. Does
    /// nothing without a model, or while already running.
    pub(super) fn transcribe_pending(self: &Rc<Self>) {
        if self.transcribing.get() || !kollate_transcribe::supported() {
            return;
        }
        let Some(model) = self.chosen_model() else {
            return;
        };
        let jobs = match self.lib.borrow().pending_transcriptions(model.model.id) {
            Ok(jobs) if !jobs.is_empty() => jobs,
            _ => return,
        };
        self.transcribing.set(true);
        let gpu = !self.flag(CPU_ONLY);
        let dictionary_dirs = dict::search_dirs(self.lib.borrow().assets_dir().as_deref());
        // A toast that stays while reading, counting through a long run
        // (a first import can bring hundreds of markups).
        let total = jobs.len();
        let progress = adw::Toast::builder()
            .title(format!("Reading the handwriting in {}…", what(&jobs)))
            .timeout(0)
            .build();
        self.show_toast(progress.clone());
        let this = self.clone();
        glib::spawn_future_local(async move {
            // The model loads once, on the worker, and is handed back each time.
            let mut worker: Option<(Transcriber, Vec<Dictionary>)> = None;
            let (mut done, mut failed, mut words) = (Vec::new(), 0, 0);
            // Why the last reading failed, so a run where nothing worked
            // says so instead of ending in silence.
            let mut last_error: Option<String> = None;
            for (i, job) in jobs.into_iter().enumerate() {
                if total > 1 {
                    progress.set_title(&format!("Reading handwriting… {} of {total}", i + 1));
                }
                let (to_load, dirs) = (model.clone(), dictionary_dirs.clone());
                let taken = worker.take();
                let outcome = gio::spawn_blocking(move || {
                    let (mut reader, dicts) = match taken {
                        Some(w) => w,
                        None => (to_load.load(gpu)?, dict::open_all(&dirs)),
                    };
                    let known =
                        |w: &str| dicts.iter().any(|d| d.lookup(w).ok().flatten().is_some());
                    let known: Option<&dyn Fn(&str) -> bool> =
                        (!dicts.is_empty()).then_some(&known);
                    let result = job.run(&mut reader, known);
                    Ok::<_, kollate_core::Error>(((reader, dicts), job, result))
                })
                .await;
                match outcome {
                    Ok(Ok((w, job, result))) => {
                        worker = Some(w);
                        match result.and_then(|t| {
                            this.lib
                                .borrow()
                                .save_transcription(&job, &t, model.model.id)
                        }) {
                            Ok(added) => {
                                words += added;
                                done.push(job);
                            }
                            Err(err) => {
                                eprintln!(
                                    "kollate: couldn’t read markup {}: {err}",
                                    job.annotation_id
                                );
                                last_error = Some(err.to_string());
                                failed += 1;
                            }
                        }
                    }
                    Ok(Err(err)) => {
                        this.error("Couldn’t Load the Handwriting Model", err);
                        break;
                    }
                    Err(_) => {
                        // The worker thread stopped (it panicked): often the
                        // graphics card running out of memory.
                        eprintln!("kollate: the handwriting reader stopped unexpectedly");
                        last_error = Some(
                            "The reader stopped unexpectedly. If another program is using the \
                             graphics card, close it and try again, or turn off Use Graphics \
                             Card in Preferences."
                                .to_owned(),
                        );
                        failed += 1;
                        break;
                    }
                }
            }
            this.transcribing.set(false);
            progress.dismiss();
            // Circled words just added to Vocabulary need definitions.
            if words > 0 {
                this.enrich_vocab();
            }
            this.reload();
            this.update_counts();
            if !done.is_empty() {
                let mut message = format!("Read the handwriting in {}", what(&done));
                if words > 0 {
                    message.push_str(&format!(
                        ", and added {} to Vocabulary",
                        plural(words, "circled word", "circled words")
                    ));
                }
                if failed > 0 {
                    message.push_str(&format!(" ({failed} couldn’t be read)"));
                }
                this.toast(&message);
            } else if let Some(err) = last_error {
                this.error("Couldn’t Read Your Handwriting", err);
            }
        });
    }

    pub(super) fn handwriting_group(
        self: &Rc<Self>,
        dialog: &adw::PreferencesDialog,
    ) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Handwriting")
            .description(
                "Kollate can read your stylus notes, and the text you underline or circle, on this \
                 computer. Nothing is sent anywhere. Download a model’s two files, then add them with +.",
            )
            .build();
        let Some(dir) = self.models_dir() else {
            return group;
        };
        if !kollate_transcribe::supported() {
            group.add(
                &adw::ActionRow::builder()
                    .title("Not Available on This Computer")
                    .subtitle("Reading handwriting needs a processor from about 2013 or later (with AVX2).")
                    .build(),
            );
            return group;
        }
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add downloaded model files")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        add.update_property(&[gtk::accessible::Property::Label("Add Model Files")]);
        group.set_header_suffix(Some(&add));
        let weak = Rc::downgrade(self);
        add.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |_| {
                if let Some(this) = weak.upgrade() {
                    this.add_models(&dialog);
                }
            }
        ));

        let installed = kollate_transcribe::installed(&dir);
        let chosen = self.chosen_model().map(|m| m.model.id);
        let mut radio_group: Option<gtk::CheckButton> = None;
        for model in catalog() {
            let have = installed.iter().any(|i| i.model.id == model.id);
            let mut subtitle = vec![format!("{:.1} GB", model.size() as f64 / 1e9)];
            if model.recommended {
                subtitle.insert(0, "Recommended".into());
            }
            // With one model installed there's nothing to choose between: a lone
            // radio button would draw as a checkbox, so say it's in use instead.
            let only = have && installed.len() == 1;
            if only {
                subtitle.insert(0, "In use".into());
            }
            subtitle.push(model.summary.into());
            let row = adw::ExpanderRow::builder()
                .title(model.name)
                .subtitle(subtitle.join(" · "))
                .build();
            if have && !only {
                let pick = gtk::CheckButton::builder()
                    .active(chosen == Some(model.id))
                    .valign(gtk::Align::Center)
                    .tooltip_text("Use this model")
                    .build();
                pick.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Use {}",
                    model.name
                ))]);
                pick.set_group(radio_group.as_ref());
                radio_group.get_or_insert_with(|| pick.clone());
                let weak = Rc::downgrade(self);
                let id = model.id;
                pick.connect_toggled(move |b| {
                    let Some(this) = weak.upgrade() else { return };
                    if b.is_active() {
                        if let Err(err) = this.lib.borrow().set_setting(MODEL_KEY, id) {
                            this.error("Couldn’t Save Preference", err);
                        }
                        this.transcribe_pending();
                    }
                });
                row.add_prefix(&pick);
            }
            if have {
                let files = adw::ActionRow::builder()
                    .title("Installed")
                    .subtitle(format!(
                        "{} and {}",
                        model.file(Role::Model).name,
                        model.file(Role::Vision).name
                    ))
                    .build();
                let remove = gtk::Button::builder()
                    .label("Remove")
                    .valign(gtk::Align::Center)
                    .css_classes(["destructive-action"])
                    .build();
                remove.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Remove {}",
                    model.name
                ))]);
                files.add_suffix(&remove);
                let weak = Rc::downgrade(self);
                let (dir, model) = (dir.clone(), *model);
                remove.connect_clicked(glib::clone!(
                    #[weak]
                    dialog,
                    move |_| {
                        let Some(this) = weak.upgrade() else { return };
                        if let Err(err) = kollate_transcribe::remove(&dir, &model) {
                            this.error("Couldn’t Remove Model", err);
                        }
                        dialog.close();
                        this.show_preferences();
                    }
                ));
                row.add_row(&files);
            } else {
                for (role, title) in [
                    (Role::Model, "Download Model File"),
                    (Role::Vision, "Download Vision File"),
                ] {
                    let file = model.file(role);
                    let link = adw::ActionRow::builder()
                        .title(title)
                        .subtitle(format!("{} · {:.1} GB", file.name, file.size as f64 / 1e9))
                        .activatable(true)
                        .build();
                    link.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
                    let weak = Rc::downgrade(self);
                    let url = file.url;
                    link.connect_activated(move |_| {
                        if let Some(this) = weak.upgrade() {
                            gtk::UriLauncher::new(url).launch(
                                Some(&this.win),
                                gio::Cancellable::NONE,
                                |_| {},
                            );
                        }
                    });
                    row.add_row(&link);
                }
            }
            group.add(&row);
        }

        let gpu = adw::SwitchRow::builder()
            .title("Use Graphics Card")
            .subtitle("Much faster when there is one. Turn off if reading handwriting fails.")
            .active(!self.flag(CPU_ONLY))
            .build();
        let weak = Rc::downgrade(self);
        gpu.connect_active_notify(move |row| {
            if let Some(this) = weak.upgrade() {
                this.set_flag(CPU_ONLY, !row.is_active());
            }
        });
        group.add(&gpu);

        // On unless turned off (SPEC §8c).
        let setting = kollate_core::store::GLOSSES_SETTING;
        let glosses = adw::SwitchRow::builder()
            .title("Circled Words Go to Vocabulary")
            .subtitle("A single word you circle on a page is added with its sentence. A note written beside it becomes your gloss.")
            .active(self.lib.borrow().setting(setting).ok().flatten().as_deref() != Some("0"))
            .build();
        let weak = Rc::downgrade(self);
        glosses.connect_active_notify(move |row| {
            if let Some(this) = weak.upgrade() {
                this.set_flag(setting, row.is_active());
            }
        });
        group.add(&glosses);
        group
    }

    /// Checks and copies model files the user downloaded, then reads any
    /// handwriting waiting for a model.
    fn add_models(self: &Rc<Self>, prefs: &adw::PreferencesDialog) {
        let Some(dir) = self.models_dir() else { return };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Model files (.gguf)"));
        filter.add_pattern("*.gguf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title("Add Model Files")
            .modal(true)
            .filters(&filters)
            .build();
        let this = self.clone();
        let prefs = prefs.clone();
        glib::spawn_future_local(async move {
            let Ok(files) = chooser.open_multiple_future(Some(&this.win)).await else {
                return;
            };
            let paths: Vec<PathBuf> = files
                .iter::<gio::File>()
                .flatten()
                .filter_map(|f| f.path())
                .collect();
            if paths.is_empty() {
                return;
            }
            this.show_toast(
                adw::Toast::builder()
                    .title("Checking and copying model files…")
                    .timeout(0)
                    .build(),
            );
            let added = gio::spawn_blocking(move || kollate_transcribe::add(&dir, &paths)).await;
            if let Some(toast) = this.last_toast.borrow().as_ref() {
                toast.dismiss();
            }
            match added {
                Ok(Ok(added)) => {
                    let name = |p: &PathBuf| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    };
                    let mut news: Vec<String> = added
                        .complete
                        .iter()
                        .map(|m| format!("Added {}", m.name))
                        .collect();
                    news.extend(
                        added.waiting.iter().map(|(m, role)| {
                            format!("{} also needs {}", m.name, m.file(*role).name)
                        }),
                    );
                    let mut problems: Vec<String> = added
                        .incomplete
                        .iter()
                        .map(|(p, file, size)| {
                            format!(
                                "{} hasn’t finished downloading ({:.1} of {:.1} GB). Add it again when it has.",
                                name(p),
                                *size as f64 / 1e9,
                                file.size as f64 / 1e9
                            )
                        })
                        .collect();
                    problems.extend(
                        added
                            .unknown
                            .iter()
                            .map(|p| format!("{} isn’t one of the offered model files.", name(p))),
                    );
                    prefs.close();
                    this.show_preferences();
                    if problems.is_empty() {
                        if !news.is_empty() {
                            this.toast(&news.join(" · "));
                        }
                    } else {
                        // Every file's outcome, not just the first.
                        news.extend(problems);
                        this.error("Some Files Weren’t Added", news.join("\n\n"));
                    }
                    this.transcribe_pending();
                }
                Ok(Err(err)) => this.error("Couldn’t Add Model", err),
                Err(_) => this.error("Couldn’t Add Model", "The copy stopped unexpectedly."),
            }
        });
    }
}

/// "2 markups", "1 notebook page", or "2 markups and 1 notebook page".
fn what(jobs: &[kollate_core::store::MarkupJob]) -> String {
    let pages = jobs.iter().filter(|j| j.page).count();
    let markups = jobs.len() - pages;
    match (markups, pages) {
        (m, 0) => plural(m, "markup", "markups"),
        (0, p) => plural(p, "notebook page", "notebook pages"),
        (m, p) => format!(
            "{} and {}",
            plural(m, "markup", "markups"),
            plural(p, "notebook page", "notebook pages")
        ),
    }
}
