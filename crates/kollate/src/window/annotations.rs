//! Highlight cards, selection, and triage and bulk actions with undo.

use super::*;

impl Window {
    pub(super) fn annotation_row(
        self: &Rc<Self>,
        a: &Annotation,
        in_book_view: bool,
    ) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::builder()
            .child(&card::build(a, in_book_view))
            .name(format!("a{}", a.id))
            .build();
        let group = gio::SimpleActionGroup::new();
        let id = a.id;
        let add = |name: &str, f: fn(&Rc<Self>, &gtk::ListBoxRow, i64)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            let row = row.downgrade();
            action.connect_activate(move |_, _| {
                if let (Some(this), Some(row)) = (weak.upgrade(), row.upgrade()) {
                    f(&this, &row, id);
                }
            });
            group.add_action(&action);
        };
        add("star", |this, row, id| {
            let before = this.lib.borrow().annotation(id).ok().flatten();
            let Some(before) = before else { return };
            // Starring something in the Inbox means you want it: keep it too.
            let keep = !before.starred && before.status == Status::Inbox;
            let result = (|| {
                let lib = this.lib.borrow();
                lib.set_starred(id, !before.starred)?;
                if keep {
                    lib.set_status(id, Status::Kept)?;
                }
                kollate_core::Result::Ok(())
            })();
            let ok = result.is_ok();
            this.after_change(row, id, result);
            if ok && keep {
                this.undo_toast(
                    "Starred and kept".to_owned(),
                    vec![(id, before.status, before.starred)],
                );
            }
        });
        add("keep", |this, row, id| {
            this.change_status(row, id, Status::Kept)
        });
        add("inbox", |this, row, id| {
            this.change_status(row, id, Status::Inbox)
        });
        add("archive", |this, row, id| {
            this.change_status(row, id, Status::Archived)
        });
        add("trash", |this, row, id| {
            this.change_status(row, id, Status::Trashed)
        });
        add("accept-device", |this, row, id| {
            let result = this.lib.borrow().accept_device_version(id);
            this.after_change(row, id, result);
        });
        add("copy", |this, _, id| {
            if let Some(text) = this
                .lib
                .borrow()
                .annotation(id)
                .ok()
                .flatten()
                .and_then(|a| a.text().map(str::to_owned))
            {
                this.win.clipboard().set_text(&text);
                this.toast("Copied");
            }
        });
        add("copy-markdown", |this, _, id| {
            if let Some(a) = this.lib.borrow().annotation(id).ok().flatten() {
                this.win.clipboard().set_text(&card::to_markdown(&a));
                this.toast("Copied as Markdown");
            }
        });
        add("edit", |this, row, id| this.edit(row, id));
        add("open-image", |this, _, id| {
            let Some(path) = this
                .lib
                .borrow()
                .annotation(id)
                .ok()
                .flatten()
                .and_then(|a| a.markup_image)
            else {
                return;
            };
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
            launcher.launch(Some(&this.win), gio::Cancellable::NONE, |_| {});
        });
        row.insert_action_group("card", Some(&group));
        row
    }

    /// IDs of the selected highlight rows, in list order.
    pub(super) fn selected_annotation_ids(&self) -> Vec<i64> {
        let mut rows = self.list.selected_rows();
        rows.sort_by_key(|r| r.index());
        rows.iter()
            .filter_map(|r| r.widget_name().strip_prefix('a')?.parse().ok())
            .collect()
    }

    /// IDs of every highlight row currently listed.
    pub(super) fn listed_annotation_ids(&self) -> Vec<i64> {
        (0..)
            .map_while(|i| self.list.row_at_index(i))
            .filter_map(|r| r.widget_name().strip_prefix('a')?.parse().ok())
            .collect()
    }

    /// Keeps only the first selected row selected.
    pub(super) fn collapse_selection(&self) {
        let mut rows = self.list.selected_rows();
        rows.sort_by_key(|r| r.index());
        if let Some(first) = rows.first() {
            self.list.unselect_all();
            self.list.select_row(Some(first));
            first.grab_focus();
        }
    }

    pub(super) fn update_selection_bar(&self) {
        let n = self.selected_annotation_ids().len();
        let show = n > 1 && matches!(self.current.get(), Nav::Annotations(_));
        self.selection_bar.set_revealed(show);
        if show {
            self.selection_label.set_label(&format!("{n} selected"));
        }
    }

    /// Applies `action` (keep, archive, trash, inbox or star) to several
    /// highlights at once, with one Undo for all of them.
    pub(super) fn bulk(self: &Rc<Self>, action: &str, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let before: Vec<(i64, Status, bool)> = {
            let lib = self.lib.borrow();
            ids.iter()
                .filter_map(|id| lib.annotation(*id).ok().flatten())
                .map(|a| (a.id, a.status, a.starred))
                .collect()
        };
        // Star all, unless they're all starred already (then unstar all).
        let star = !before.iter().all(|(_, _, starred)| *starred);
        let result = (|| {
            let lib = self.lib.borrow();
            for (id, status, _) in &before {
                match action {
                    "keep" => lib.set_status(*id, Status::Kept)?,
                    "archive" => lib.set_status(*id, Status::Archived)?,
                    "trash" => lib.set_status(*id, Status::Trashed)?,
                    "inbox" => lib.set_status(*id, Status::Inbox)?,
                    "star" => {
                        lib.set_starred(*id, star)?;
                        if star && *status == Status::Inbox {
                            lib.set_status(*id, Status::Kept)?;
                        }
                    }
                    _ => {}
                }
            }
            kollate_core::Result::Ok(())
        })();
        if let Err(err) = result {
            self.error("Couldn’t Update Highlights", err);
        }
        let n = before.len();
        let what = plural(n, "highlight", "highlights");
        let message = match action {
            "keep" => format!("Kept {what}"),
            "archive" => format!("Archived {what}"),
            "trash" => format!("Moved {what} to Trash"),
            "inbox" => format!("Moved {what} to the Inbox"),
            "star" if star => format!("Starred {what}"),
            _ => format!("Unstarred {what}"),
        };
        // Reload, keeping the position of the first affected row.
        let first_index = ids.first().and_then(|id| {
            (0..)
                .map_while(|i| self.list.row_at_index(i))
                .position(|r| r.widget_name() == format!("a{id}"))
        });
        self.reload();
        if let Some(i) = first_index {
            let row = self.list.row_at_index(i as i32).or_else(|| {
                let last = (0..).map_while(|i| self.list.row_at_index(i)).count() as i32 - 1;
                self.list.row_at_index(last)
            });
            if let Some(row) = row.filter(|r| r.is_selectable()) {
                self.list.unselect_all();
                self.list.select_row(Some(&row));
                row.grab_focus();
            }
        }
        self.update_counts();
        self.undo_toast(message, before);
    }

    /// A toast whose Undo puts each highlight's status and star back.
    fn undo_toast(self: &Rc<Self>, message: String, before: Vec<(i64, Status, bool)>) {
        let toast = adw::Toast::builder()
            .title(message)
            .button_label("Undo")
            .timeout(5)
            .build();
        let weak = Rc::downgrade(self);
        toast.connect_button_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            {
                let lib = this.lib.borrow();
                for (id, status, starred) in &before {
                    let _ = lib.set_status(*id, *status);
                    let _ = lib.set_starred(*id, *starred);
                }
            }
            this.reload();
            this.update_counts();
        });
        self.show_toast(toast);
    }

    fn change_status(self: &Rc<Self>, row: &gtk::ListBoxRow, id: i64, status: Status) {
        let previous = self
            .lib
            .borrow()
            .annotation(id)
            .ok()
            .flatten()
            .map(|a| a.status);
        if previous == Some(status) {
            return;
        }
        let result = self.lib.borrow().set_status(id, status);
        let ok = result.is_ok();
        self.after_change(row, id, result);
        let (Some(previous), true) = (previous, ok) else {
            return;
        };
        let message = match status {
            Status::Kept if previous == Status::Inbox => "Kept",
            Status::Kept => "Restored",
            Status::Inbox => "Moved to Inbox",
            Status::Archived => "Archived",
            Status::Trashed => "Moved to Trash",
        };
        let toast = adw::Toast::builder()
            .title(message)
            .button_label("Undo")
            .timeout(4)
            .build();
        let weak = Rc::downgrade(self);
        toast.connect_button_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let _ = this.lib.borrow().set_status(id, previous);
                this.reload();
                this.update_counts();
            }
        });
        self.show_toast(toast);
    }

    pub(super) fn edit(self: &Rc<Self>, row: &gtk::ListBoxRow, id: i64) {
        let Some(a) = self.lib.borrow().annotation(id).ok().flatten() else {
            return;
        };
        let weak = Rc::downgrade(self);
        let row = row.downgrade();
        let (user_text, user_note, tags) =
            (a.user_text.clone(), a.user_note.clone(), a.tags.clone());
        edit::present(&self.win, &a, move |edited| {
            let (Some(this), Some(row)) = (weak.upgrade(), row.upgrade()) else {
                return;
            };
            let tags_changed = edited.tags != tags;
            let result = (|| {
                let mut lib = this.lib.borrow_mut();
                if edited.text != user_text {
                    lib.set_user_text(id, edited.text.as_deref())?;
                }
                if edited.note != user_note {
                    lib.set_user_note(id, edited.note.as_deref())?;
                }
                if tags_changed {
                    let tags: Vec<&str> = edited.tags.iter().map(String::as_str).collect();
                    lib.set_annotation_tags(id, &tags)?;
                }
                Ok(())
            })();
            this.after_change(&row, id, result);
            if tags_changed {
                this.rebuild_sidebar();
            }
        });
    }

    /// Refreshes one card after a change, or removes it if it no longer
    /// belongs in the current view, then moves the selection along.
    fn after_change(
        self: &Rc<Self>,
        row: &gtk::ListBoxRow,
        id: i64,
        result: kollate_core::Result<()>,
    ) {
        if let Err(err) = result {
            self.error("Couldn’t Save Change", err);
            return;
        }
        let Nav::Annotations(view) = self.current.get() else {
            return;
        };
        let filter = AnnotationFilter {
            view,
            search: self.search_text(),
            id: Some(id),
        };
        let still_here = self
            .lib
            .borrow()
            .query_annotations(&filter)
            .ok()
            .and_then(|v| v.into_iter().next());
        match still_here {
            Some(a) => {
                row.set_child(Some(&card::build(&a, matches!(view, View::Book(_)))));
                row.grab_focus();
            }
            None => {
                let index = row.index();
                self.list.remove(row);
                if index >= 0 {
                    self.groups.borrow_mut().remove(index as usize);
                }
                let (annotations, words) = self.shown.get();
                self.shown.set((annotations.saturating_sub(1), words));
                self.list.invalidate_headers();
                let next = self
                    .list
                    .row_at_index(index)
                    .or_else(|| self.list.row_at_index(index - 1));
                if let Some(next) = next {
                    self.list.unselect_all();
                    self.list.select_row(Some(&next));
                    next.grab_focus();
                }
                self.update_title();
                self.update_empty_state();
            }
        }
        self.update_counts();
    }
}
