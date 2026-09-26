//! The content pane: titles, filling the list for each view, and vocabulary rows.

use super::*;

impl Window {
    fn nav_title(&self, nav: Nav) -> (String, Option<String>) {
        match nav {
            Nav::Annotations(View::Book(id)) => {
                self.books.borrow().get(&id).cloned().unwrap_or_default()
            }
            Nav::Annotations(View::Tag(id)) => (
                self.tags.borrow().get(&id).cloned().unwrap_or_default(),
                None,
            ),
            _ => {
                let label = TOP_LEVEL
                    .iter()
                    .find(|(n, _, _)| *n == nav)
                    .map_or("", |(_, _, l)| l);
                (label.to_owned(), None)
            }
        }
    }

    pub(super) fn update_title(&self) {
        let nav = self.current.get();
        let (title, author) = self.nav_title(nav);
        let (annotations, words) = self.shown.get();
        let mut parts: Vec<String> = author.into_iter().collect();
        if nav == Nav::Books && self.books_shown.get() > 0 {
            parts.push(plural(self.books_shown.get(), "book", "books"));
        }
        if annotations > 0 {
            parts.push(plural(annotations, "highlight", "highlights"));
        }
        if words > 0 {
            parts.push(plural(words, "word", "words"));
        }
        self.content_page.set_title(&title);
        self.title.set_title(&title);
        self.title.set_subtitle(&parts.join(" · "));
    }

    pub(super) fn search_text(&self) -> Option<String> {
        Some(self.search.text().to_string()).filter(|s| !s.trim().is_empty())
    }

    /// Reloads the content pane for the current view.
    pub(super) fn reload(self: &Rc<Self>) {
        self.list.remove_all();
        self.groups.borrow_mut().clear();
        self.shown.set((0, 0));
        let nav = self.current.get();
        let separate = !matches!(nav, Nav::Vocab);
        self.list.set_css_classes(if separate {
            &["boxed-list-separate", "annotation-list"]
        } else {
            &["boxed-list", "annotation-list"]
        });

        self.back
            .set_visible(self.from_books.get() && matches!(nav, Nav::Annotations(View::Book(_))));
        self.book_sort.set_visible(nav == Nav::Books);
        self.search.set_placeholder_text(Some(match nav {
            Nav::Books => "Search books by title or author",
            Nav::Vocab => "Search words, definitions and sentences",
            _ => "Search highlights, notes, chapters, books and tags",
        }));
        let result = match nav {
            Nav::Annotations(view) => self.fill_annotations(view),
            Nav::Vocab => self.fill_vocab(None, "").map(|_| ()),
            Nav::Books => self.fill_books(),
        };
        if let Err(err) = result {
            self.error("Couldn’t Load Library", err);
        }
        self.update_title();
        self.update_empty_state();
        let inbox_has_items = nav == Nav::Annotations(View::Inbox) && self.shown.get().0 > 0;
        self.keep_all.set_visible(inbox_has_items);
        let tip_dismissed = self
            .lib
            .borrow()
            .setting("tip_triage_dismissed")
            .ok()
            .flatten()
            .is_some();
        self.tip.set_revealed(inbox_has_items && !tip_dismissed);
        self.selection_bar.set_revealed(false);
        let first = (0..)
            .map_while(|i| self.list.row_at_index(i))
            .find(|r| r.is_selectable());
        self.list.select_row(first.as_ref());
    }

    fn fill_annotations(self: &Rc<Self>, view: View) -> kollate_core::Result<()> {
        let filter = AnnotationFilter {
            view,
            search: self.search_text(),
            id: None,
        };
        let items = self.lib.borrow().query_annotations(&filter)?;
        let in_book_view = matches!(view, View::Book(_));
        if let View::Book(id) = view
            && self.search_text().is_none()
            && let Some(book) = self.lib.borrow().book(id)?
        {
            self.groups.borrow_mut().push(String::new());
            self.list.append(&card::book_header(&book));
        }
        for a in &items {
            let group = if !in_book_view {
                match &a.book_author {
                    Some(author) => format!("{} — {author}", a.book_title),
                    None => a.book_title.clone(),
                }
            } else {
                a.chapter_title.clone().unwrap_or_default()
            };
            self.groups.borrow_mut().push(group);
            self.list.append(&self.annotation_row(a, in_book_view));
        }
        self.shown.set((items.len(), 0));
        if let View::Book(id) = view {
            self.fill_vocab(Some(id), "Vocabulary")?;
        }
        Ok(())
    }

    fn fill_vocab(self: &Rc<Self>, book: Option<i64>, group: &str) -> kollate_core::Result<usize> {
        let words = self
            .lib
            .borrow()
            .query_vocab(book, self.search_text().as_deref())?;
        for v in &words {
            self.groups.borrow_mut().push(group.to_owned());
            self.list.append(&self.vocab_row(v, book.is_none()));
        }
        let (annotations, _) = self.shown.get();
        self.shown.set((annotations, words.len()));
        Ok(words.len())
    }

    fn fill_books(self: &Rc<Self>) -> kollate_core::Result<()> {
        while let Some(child) = self.books_grid.first_child() {
            self.books_grid.remove(&child);
        }
        let sort = BookSort::ALL[(self.book_sort.selected() as usize).min(BookSort::ALL.len() - 1)];
        let books = self
            .lib
            .borrow()
            .shelf(sort, self.search_text().as_deref())?;
        for b in &books {
            let tile = card::book_tile(b);
            if let Some(button) = tile.child().and_downcast::<gtk::Button>() {
                let weak = Rc::downgrade(self);
                let id = b.id;
                button.connect_clicked(move |_| {
                    if let Some(this) = weak.upgrade() {
                        this.navigate(Nav::Annotations(View::Book(id)), true);
                    }
                });
            }
            self.books_grid.append(&tile);
        }
        self.books_shown.set(books.len());
        Ok(())
    }

    pub(super) fn update_empty_state(&self) {
        let on_books = self.current.get() == Nav::Books;
        let empty = if on_books {
            self.books_shown.get() == 0
        } else {
            self.shown.get() == (0, 0)
        };
        let page = match (empty, on_books) {
            (true, _) => "empty",
            (false, true) => "books",
            (false, false) => "list",
        };
        self.stack.set_visible_child_name(page);
        if !empty {
            return;
        }
        let lib_empty = self
            .lib
            .borrow()
            .books()
            .map(|b| b.is_empty())
            .unwrap_or(false);
        let (icon, title, description) = if self.search_text().is_some() {
            (
                "system-search-symbolic",
                "No Results",
                "Try a different search.",
            )
        } else if lib_empty {
            (
                "media-removable-symbolic",
                "Welcome to Kollate",
                "Connect your Kobo with a USB cable, then choose Import (Ctrl+I). Nothing on the Kobo is ever changed.",
            )
        } else {
            match self.current.get() {
                Nav::Annotations(View::Inbox) => (
                    "mail-unread-symbolic",
                    "Inbox Zero",
                    "New highlights from your Kobo will appear here.",
                ),
                Nav::Annotations(View::Starred) => (
                    "starred-symbolic",
                    "No Starred Highlights",
                    "Press S on a highlight to star it.",
                ),
                Nav::Annotations(View::Trash) => ("user-trash-symbolic", "Trash Is Empty", ""),
                Nav::Books => (
                    "folder-documents-symbolic",
                    "No Books",
                    "Books with highlights or looked-up words appear here. Books whose highlights are all archived or trashed are hidden.",
                ),
                Nav::Annotations(View::RemovedOnDevice) => (
                    "edit-delete-symbolic",
                    "Nothing Deleted on Your Kobo",
                    "Highlights you delete on your Kobo stay in Kollate and show up here.",
                ),
                Nav::Vocab => (
                    "accessories-dictionary-symbolic",
                    "No Words Yet",
                    "Words you look up on your Kobo appear here.",
                ),
                _ => ("view-list-bullet-symbolic", "Nothing Here", ""),
            }
        };
        self.empty.set_icon_name(Some(icon));
        self.empty.set_title(title);
        self.empty
            .set_description(Some(description).filter(|d| !d.is_empty()));
    }

    fn vocab_row(self: &Rc<Self>, v: &Vocab, show_books: bool) -> gtk::ListBoxRow {
        let hbox = gtk::Box::builder()
            .spacing(12)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(14)
            .margin_end(10)
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .hexpand(true)
            .build();
        let label = |markup: &str, css: &[&str]| {
            gtk::Label::builder()
                .use_markup(true)
                .label(markup)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .css_classes(css.iter().map(|c| c.to_string()).collect::<Vec<_>>())
                .build()
        };
        let mut title = format!("<b>{}</b>", glib::markup_escape_text(&v.word));
        if let Some(lemma) = v
            .lemma
            .as_deref()
            .filter(|l| !l.eq_ignore_ascii_case(&v.word))
        {
            title.push_str(&format!(
                "  <span alpha=\"60%\">{}</span>",
                glib::markup_escape_text(lemma)
            ));
        }
        text.append(&label(&title, &[]));
        match v.definition.as_deref().and_then(|d| d.lines().next()) {
            Some(first) => {
                let def = label(&glib::markup_escape_text(first), &[]);
                def.set_lines(2);
                def.set_ellipsize(gtk::pango::EllipsizeMode::End);
                text.append(&def);
            }
            None => text.append(&label("No definition yet", &["dim-label"])),
        }
        if let Some(context) = &v.context {
            let words = [v.word.as_str(), v.lemma.as_deref().unwrap_or("")];
            let ctx = label(
                &word::context_markup(context, &words),
                &["quote", "dim-label"],
            );
            ctx.set_lines(3);
            ctx.set_ellipsize(gtk::pango::EllipsizeMode::End);
            ctx.set_margin_top(2);
            text.append(&ctx);
        }
        let mut meta = Vec::new();
        if show_books && !v.books.is_empty() {
            meta.push(v.books.join(", "));
        }
        if let Some(date) = v.first_seen_at {
            meta.push(card::format_date(date));
        }
        if !meta.is_empty() {
            let m = label(
                &glib::markup_escape_text(&meta.join(" · ")),
                &["caption", "dim-label"],
            );
            m.set_margin_top(2);
            text.append(&m);
        }
        hbox.append(&text);

        let status = gtk::DropDown::from_strings(&VOCAB_STATUS_LABELS);
        status.set_valign(gtk::Align::Center);
        status.add_css_class("flat");
        status.set_tooltip_text(Some("Learning status"));
        let index = VocabStatus::ALL
            .iter()
            .position(|s| *s == v.status)
            .unwrap_or(0);
        status.set_selected(index as u32);
        let weak = Rc::downgrade(self);
        let id = v.id;
        status.connect_selected_notify(move |dd| {
            let Some(this) = weak.upgrade() else { return };
            let status = VocabStatus::ALL[(dd.selected() as usize).min(3)];
            if let Err(err) = this.lib.borrow().set_vocab_status(id, status) {
                this.error("Couldn’t Update Word", err);
            }
        });
        hbox.append(&status);

        let row = gtk::ListBoxRow::builder().child(&hbox).build();
        let group = gio::SimpleActionGroup::new();
        let edit = gio::SimpleAction::new("edit", None);
        let weak = Rc::downgrade(self);
        edit.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.open_word(id);
            }
        });
        group.add_action(&edit);
        row.insert_action_group("card", Some(&group));
        row
    }

    fn open_word(self: &Rc<Self>, id: i64) {
        let detail = match self.lib.borrow().vocab_detail(id) {
            Ok(Some(d)) => d,
            Ok(None) => return,
            Err(err) => return self.error("Couldn’t Open Word", err),
        };
        let weak = Rc::downgrade(self);
        let look_up_again = move || {
            let this = weak.upgrade()?;
            let dicts = this.dictionaries.borrow();
            let mut lib = this.lib.borrow_mut();
            lib.set_vocab_definition(id, None).ok()?;
            lib.enrich_definitions(&dicts).ok()?;
            lib.vocab_detail(id).ok()??.vocab.definition
        };
        let weak = Rc::downgrade(self);
        word::present(&self.win, &detail, look_up_again, move |edit| {
            let Some(this) = weak.upgrade() else { return };
            let result = (|| {
                let lib = this.lib.borrow();
                if let Some(def) = &edit.definition {
                    lib.set_vocab_definition(id, Some(def))?;
                }
                for (sighting, context) in &edit.contexts {
                    lib.set_sighting_context(*sighting, context.as_deref())?;
                }
                kollate_core::Result::Ok(())
            })();
            if let Err(err) = result {
                this.error("Couldn’t Save Word", err);
            }
            this.reload();
        });
    }

    /// (Re)opens all installed and imported dictionaries.
    pub(super) fn load_dictionaries(&self) {
        let mut dirs = dict::search_dirs(self.lib.borrow().assets_dir().as_deref());
        if cfg!(debug_assertions) {
            // Development builds use the WordNet built by scripts/fetch-wordnet.sh.
            dirs.push(PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../data/dictionaries"
            )));
        }
        *self.dictionaries.borrow_mut() = dict::open_all(&dirs);
    }

    /// Defines any words still missing a definition. Returns how many were defined.
    pub(super) fn enrich_vocab(&self) -> usize {
        let dicts = self.dictionaries.borrow();
        if dicts.is_empty() {
            return 0;
        }
        // When the installed dictionaries change (e.g. a better one is
        // bundled or added), look up again every definition that came from
        // a dictionary. Definitions the user wrote are never touched.
        let signature: String = dicts
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join("\u{1f}");
        let previous = self.lib.borrow().setting("dictionaries").ok().flatten();
        if previous.as_deref() != Some(signature.as_str()) {
            if let Err(err) = self.lib.borrow_mut().refresh_definitions(&dicts) {
                self.error("Couldn’t Look Up Words", err);
            }
            let _ = self.lib.borrow().set_setting("dictionaries", &signature);
        }
        match self.lib.borrow_mut().enrich_definitions(&dicts) {
            Ok(n) => n,
            Err(err) => {
                self.error("Couldn’t Look Up Words", err);
                0
            }
        }
    }
}
