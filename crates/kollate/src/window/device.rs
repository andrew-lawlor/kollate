//! Importing, and noticing, importing from and ejecting a connected Kobo.

use super::*;

impl Window {
    pub(super) fn choose_import_folder(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title("Choose Your Kobo")
            .modal(true)
            .build();
        let media = PathBuf::from("/media").join(std::env::var("USER").unwrap_or_default());
        if media.is_dir() {
            dialog.set_initial_folder(Some(&gio::File::for_path(media)));
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(folder) = dialog.select_folder_future(Some(&this.win)).await
                && let Some(path) = folder.path()
            {
                this.import_from(path);
            }
        });
    }

    /// Imports from a Kobo mount point (or a folder/DB file) in the
    /// background: reads a copy of the database and copies covers and
    /// markup images, then merges everything into the library.
    pub(super) fn import_from(self: &Rc<Self>, path: PathBuf) {
        if self.importing.replace(true) {
            return;
        }
        let this = self.clone();
        let is_connected = self.kobo.borrow().as_ref().is_some_and(|k| k.root == path);
        if is_connected {
            self.set_banner(&format!("Importing from your {}…", self.kobo_name()), None);
        } else {
            self.show_toast(adw::Toast::builder().title("Importing…").timeout(0).build());
        }
        let assets_dir = self.lib.borrow().assets_dir();
        glib::spawn_future_local(async move {
            let read = gio::spawn_blocking(move || -> kollate_core::Result<_> {
                let device = DeviceInfo::identify(&path)?;
                let snapshot = KoboDb::open_copy(&find_kobo_db(&path)?)?.snapshot()?;
                // Asset copying is best-effort; a failure never blocks the import.
                let assets = match (&assets_dir, path.is_dir()) {
                    (Some(dir), true) => {
                        copy_assets(&path, &snapshot, dir).map_err(|e| e.to_string())
                    }
                    _ => Ok(CopiedAssets::default()),
                };
                // Context sentences come from the books themselves.
                let contexts = if path.is_dir() {
                    find_word_contexts(&path, &snapshot, 5)
                } else {
                    Vec::new()
                };
                Ok((device, snapshot, assets, contexts))
            })
            .await;
            this.importing.set(false);
            if let Some(toast) = this.last_toast.borrow().as_ref() {
                toast.dismiss();
            }
            let outcome = match read {
                Ok(Ok((device, snapshot, assets, contexts))) => {
                    let mut lib = this.lib.borrow_mut();
                    lib.import(&snapshot, &device, false)
                        .and_then(|stats| {
                            if let Ok(assets) = &assets {
                                lib.attach_assets(&device, assets)?;
                            }
                            lib.set_word_contexts(&device, &contexts)?;
                            Ok((stats, assets.err(), device, snapshot.db_version))
                        })
                        .map_err(|e| e.to_string())
                }
                Ok(Err(err)) => Err(err.to_string()),
                Err(_) => Err("The import stopped unexpectedly.".to_owned()),
            };
            match outcome {
                Ok((stats, asset_error, device, db_version)) => {
                    this.enrich_vocab();
                    this.rebuild_sidebar();
                    this.reload();
                    this.import_toast(&stats);
                    this.warn_if_untested(&device, db_version);
                    this.auto_sync_obsidian();
                    if let Some(err) = asset_error {
                        this.toast(&format!("Some images couldn’t be copied: {err}"));
                    }
                }
                Err(err) => this.error("Import Failed", err),
            }
            if is_connected {
                this.show_connected_banner();
            }
        });
    }

    /// Imports from Kobo database versions Kollate hasn't been tested with
    /// go ahead, with a one-time toast (per version) asking for a report.
    /// It's queued behind the import summary rather than replacing it.
    fn warn_if_untested(self: &Rc<Self>, device: &DeviceInfo, db_version: i64) {
        const KEY: &str = "warned_db_version";
        let lib = self.lib.borrow();
        if is_tested_db_version(db_version)
            || lib.setting(KEY).ok().flatten() == Some(db_version.to_string())
        {
            return;
        }
        let _ = lib.set_setting(KEY, &db_version.to_string());
        let toast = adw::Toast::builder()
            .title(format!(
                "This Kobo’s database (version {db_version}) hasn’t been tested yet. Please check your highlights."
            ))
            .button_label("Report")
            .timeout(20)
            .build();
        let url = compatibility_report_url(device, db_version);
        let weak = Rc::downgrade(self);
        toast.connect_button_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                gtk::UriLauncher::new(&url).launch(Some(&this.win), gio::Cancellable::NONE, |_| {});
            }
        });
        self.toasts.add_toast(toast);
    }

    // ---- Device ------------------------------------------------------------

    fn kobo_name(&self) -> &'static str {
        self.kobo.borrow().as_ref().map_or("Kobo", |k| k.name)
    }

    fn set_banner(&self, title: &str, action: Option<BannerAction>) {
        self.banner.set_title(title);
        self.banner.set_button_label(action.map(|a| match a {
            BannerAction::Import => "Import",
            BannerAction::Eject => "Eject",
        }));
        if let Some(action) = action {
            self.banner_action.set(action);
        }
        self.banner.set_revealed(true);
    }

    fn show_connected_banner(&self) {
        let title = format!("Your {} is connected", self.kobo_name());
        self.set_banner(&title, Some(BannerAction::Eject));
    }

    pub(super) fn on_connect_setting(&self) -> OnConnect {
        OnConnect::from_key(
            self.lib
                .borrow()
                .setting("on_connect")
                .ok()
                .flatten()
                .as_deref(),
        )
    }

    pub(super) fn setup_device_monitor(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.banner.connect_button_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            match this.banner_action.get() {
                BannerAction::Import => {
                    if let Some(root) = this.kobo.borrow().as_ref().map(|k| k.root.clone()) {
                        this.import_from(root);
                    }
                }
                BannerAction::Eject => this.eject(),
            }
        });

        let weak = Rc::downgrade(self);
        self.monitor.connect_mount_added(move |_, mount| {
            if let Some(this) = weak.upgrade() {
                this.mount_added(mount);
            }
        });
        let weak = Rc::downgrade(self);
        self.monitor.connect_mount_removed(move |_, mount| {
            let Some(this) = weak.upgrade() else { return };
            let root = mount.root().path();
            let ours = this
                .kobo
                .borrow()
                .as_ref()
                .is_some_and(|k| Some(&k.root) == root.as_ref());
            if ours {
                this.kobo.replace(None);
                this.banner.set_revealed(false);
            }
        });
        for mount in self.monitor.mounts() {
            self.mount_added(&mount);
        }
    }

    fn mount_added(self: &Rc<Self>, mount: &gio::Mount) {
        let Some(root) = mount.root().path() else {
            return;
        };
        if !is_kobo_mount(&root) || self.kobo.borrow().is_some() {
            return;
        }
        let name = DeviceInfo::read(&root).map_or("Kobo", |info| info.model_name());
        self.kobo.replace(Some(Connected {
            mount: mount.clone(),
            root: root.clone(),
            name,
        }));
        match self.on_connect_setting() {
            OnConnect::Import => self.import_from(root),
            OnConnect::Ask => self.set_banner(
                &format!("Your {name} is connected"),
                Some(BannerAction::Import),
            ),
            OnConnect::Nothing => {}
        }
    }

    fn eject(self: &Rc<Self>) {
        if self.importing.get() {
            return;
        }
        let Some(mount) = self.kobo.borrow().as_ref().map(|k| k.mount.clone()) else {
            return;
        };
        let name = self.kobo_name();
        self.set_banner(&format!("Ejecting your {name}…"), None);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let operation = gtk::MountOperation::new(Some(&this.win));
            let result = if mount.can_eject() {
                mount
                    .eject_with_operation_future(gio::MountUnmountFlags::NONE, Some(&operation))
                    .await
            } else {
                mount
                    .unmount_with_operation_future(gio::MountUnmountFlags::NONE, Some(&operation))
                    .await
            };
            match result {
                Ok(()) => {
                    this.kobo.replace(None);
                    this.banner.set_revealed(false);
                    this.toast(&format!("You can unplug your {name}"));
                }
                Err(err) => {
                    this.show_connected_banner();
                    this.error("Couldn’t Eject", err.message());
                }
            }
        });
    }

    fn import_toast(self: &Rc<Self>, s: &ImportStats) {
        let (title, target) = import_summary(s);
        let toast = adw::Toast::builder().title(title).timeout(6).build();
        if let Some((label, view)) = target {
            toast.set_button_label(Some(label));
            let weak = Rc::downgrade(self);
            toast.connect_button_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.select_nav(Nav::Annotations(view));
                }
            });
        }
        self.show_toast(toast);
    }
}

/// A new "Kobo compatibility" issue with the device details filled in.
fn compatibility_report_url(device: &DeviceInfo, db_version: i64) -> String {
    let esc = |s: &str| glib::Uri::escape_string(s, None, false).to_string();
    let model = match device.model_name() {
        "Kobo" => device.model_id.clone().unwrap_or_default(),
        name => name.to_owned(),
    };
    format!(
        "{ISSUES_URL}/new?template=compatibility.yml&model={}&firmware={}&db_version={db_version}",
        esc(&model),
        esc(device.firmware.as_deref().unwrap_or("")),
    )
}

const ISSUES_URL: &str = "https://github.com/andrew-lawlor/kollate/issues";

/// The import toast's text, and which view its button opens (if any).
fn import_summary(s: &ImportStats) -> (String, Option<(&'static str, View)>) {
    let mut parts = Vec::new();
    if s.annotations_new > 0 {
        parts.push(plural(s.annotations_new, "new highlight", "new highlights"));
    }
    if s.words_new > 0 {
        parts.push(plural(s.words_new, "new word", "new words"));
    }
    if s.annotations_updated > 0 {
        parts.push(format!("{} updated", s.annotations_updated));
    }
    if s.annotations_removed > 0 {
        parts.push(if s.annotations_trashed == s.annotations_removed {
            format!("{} deleted on Kobo, moved to Trash", s.annotations_removed)
        } else {
            format!("{} deleted on Kobo", s.annotations_removed)
        });
    }
    if s.annotations_restored > 0 {
        parts.push(format!("{} back on Kobo", s.annotations_restored));
    }
    let title = if parts.is_empty() {
        "Nothing new on your Kobo".to_owned()
    } else {
        parts.join(" · ")
    };
    // New highlights are the most useful thing to jump to; otherwise show
    // where the deleted ones went.
    let target = if s.annotations_new > 0 {
        Some(("Review", View::Inbox))
    } else if s.annotations_removed > s.annotations_trashed {
        Some(("Show", View::RemovedOnDevice))
    } else if s.annotations_trashed > 0 {
        Some(("Show", View::Trash))
    } else {
        None
    };
    (title, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_report_url_prefills_the_form() {
        let device = DeviceInfo::parse(
            "N0000000000001,4.20.14622,4.45.23697,4.20.14622,4.20.14622,00000000-0000-0000-0000-000000000390",
        )
        .unwrap();
        assert_eq!(
            compatibility_report_url(&device, 180),
            "https://github.com/andrew-lawlor/kollate/issues/new?template=compatibility.yml\
             &model=Kobo%20Libra%20Colour&firmware=4.45.23697&db_version=180"
        );
    }

    #[test]
    fn import_summary_mentions_deletions() {
        let s = ImportStats {
            annotations_removed: 2,
            ..Default::default()
        };
        assert_eq!(
            import_summary(&s),
            (
                "2 deleted on Kobo".to_owned(),
                Some(("Show", View::RemovedOnDevice))
            )
        );

        let s = ImportStats {
            annotations_removed: 1,
            annotations_trashed: 1,
            ..Default::default()
        };
        assert_eq!(
            import_summary(&s),
            (
                "1 deleted on Kobo, moved to Trash".to_owned(),
                Some(("Show", View::Trash))
            )
        );

        let s = ImportStats {
            annotations_new: 3,
            annotations_removed: 1,
            annotations_restored: 1,
            ..Default::default()
        };
        assert_eq!(
            import_summary(&s),
            (
                "3 new highlights · 1 deleted on Kobo · 1 back on Kobo".to_owned(),
                Some(("Review", View::Inbox))
            )
        );

        assert_eq!(
            import_summary(&ImportStats::default()),
            ("Nothing new on your Kobo".to_owned(), None)
        );
    }
}
