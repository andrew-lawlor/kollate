//! The sidebar: top-level views, recent books and tags, with counts.

use super::*;

impl Window {
    fn sidebar_row(
        icon: Option<&str>,
        label: &str,
        tooltip: Option<&str>,
        count: &gtk::Label,
    ) -> gtk::ListBoxRow {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        if let Some(icon) = icon {
            hbox.append(&gtk::Image::from_icon_name(icon));
        }
        hbox.append(
            &gtk::Label::builder()
                .label(label)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        count.add_css_class("dim-label");
        count.add_css_class("numeric");
        hbox.append(count);
        let row = gtk::ListBoxRow::builder().child(&hbox).build();
        row.set_tooltip_text(tooltip);
        row
    }

    fn sidebar_heading(label: &str) -> gtk::ListBoxRow {
        let label = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .margin_top(12)
            .css_classes(["heading", "dim-label"])
            .build();
        gtk::ListBoxRow::builder()
            .child(&label)
            .selectable(false)
            .activatable(false)
            .build()
    }

    pub(super) fn rebuild_sidebar(self: &Rc<Self>) {
        self.rebuilding_sidebar.set(true);
        self.sidebar.remove_all();
        let mut navs = Vec::new();
        let mut labels = HashMap::new();
        let mut add = |row: gtk::ListBoxRow, nav: Option<Nav>, count: Option<gtk::Label>| {
            self.sidebar.append(&row);
            navs.push(nav);
            if let (Some(nav), Some(count)) = (nav, count) {
                labels.insert(nav, count);
            }
        };

        for (nav, icon, label) in TOP_LEVEL {
            let count = gtk::Label::new(None);
            add(
                Self::sidebar_row(Some(icon), label, None, &count),
                Some(nav),
                Some(count),
            );
        }

        let lib = self.lib.borrow();
        // Titles of every book (for page titles); only recent ones get a row.
        let book_map: HashMap<i64, (String, Option<String>)> = lib
            .books()
            .unwrap_or_default()
            .into_iter()
            .map(|b| (b.id, (b.title, b.author)))
            .collect();
        let recent: Vec<Book> = lib
            .shelf(BookSort::Recent, None)
            .unwrap_or_default()
            .into_iter()
            .take(RECENT_BOOKS)
            .collect();
        if !recent.is_empty() {
            add(Self::sidebar_heading("Recent"), None, None);
        }
        let mut recent_ids = Vec::new();
        for b in recent {
            let count = gtk::Label::new(None);
            let tooltip = match &b.author {
                Some(author) => format!("{}\n{author}", b.title),
                None => b.title.clone(),
            };
            let nav = Nav::Annotations(View::Book(b.id));
            add(
                Self::sidebar_row(None, &b.title, Some(&tooltip), &count),
                Some(nav),
                Some(count),
            );
            recent_ids.push(b.id);
        }

        let tags = lib.tags().unwrap_or_default();
        let mut tag_map = HashMap::new();
        if !tags.is_empty() {
            add(Self::sidebar_heading("Tags"), None, None);
        }
        for t in tags {
            let count = gtk::Label::new(None);
            let nav = Nav::Annotations(View::Tag(t.id));
            add(
                Self::sidebar_row(Some("user-bookmarks-symbolic"), &t.name, None, &count),
                Some(nav),
                Some(count),
            );
            tag_map.insert(t.id, t.name);
        }
        drop(lib);

        // A book opened from the Books page has no row of its own: keep it
        // and select Books. A removed tag falls back to the Inbox.
        let current = self.current.get();
        let highlight = match current {
            _ if navs.contains(&Some(current)) => Some(current),
            Nav::Annotations(View::Book(id)) if book_map.contains_key(&id) => Some(Nav::Books),
            _ => {
                self.current.set(Nav::Annotations(View::Inbox));
                Some(Nav::Annotations(View::Inbox))
            }
        };
        let selected = navs.iter().position(|n| *n == highlight);
        *self.recent_ids.borrow_mut() = recent_ids;
        *self.sidebar_navs.borrow_mut() = navs;
        *self.count_labels.borrow_mut() = labels;
        *self.books.borrow_mut() = book_map;
        *self.tags.borrow_mut() = tag_map;
        if let Some(i) = selected {
            self.sidebar
                .select_row(self.sidebar.row_at_index(i as i32).as_ref());
        }
        self.rebuilding_sidebar.set(false);
        self.update_counts();
    }

    pub(super) fn update_counts(self: &Rc<Self>) {
        let lib = self.lib.borrow();
        let Ok(c) = lib.sidebar_counts() else { return };
        let shelf = lib.shelf(BookSort::Recent, None).unwrap_or_default();
        let recent: Vec<i64> = shelf.iter().take(RECENT_BOOKS).map(|b| b.id).collect();
        if recent != *self.recent_ids.borrow() {
            // A book gained or lost its place under Recent.
            drop(lib);
            self.rebuild_sidebar();
            return;
        }
        let mut counts: HashMap<Nav, i64> = HashMap::from([
            (Nav::Annotations(View::Inbox), c.inbox),
            (Nav::Annotations(View::All), c.all),
            (Nav::Annotations(View::Notes), c.notes),
            (Nav::Annotations(View::Markups), c.markups),
            (Nav::Annotations(View::Starred), c.starred),
            (Nav::Annotations(View::Archive), c.archive),
            (Nav::Annotations(View::Trash), c.trash),
            (Nav::Annotations(View::RemovedOnDevice), c.removed_on_device),
            (Nav::Vocab, c.vocab),
            (Nav::Books, shelf.len() as i64),
        ]);
        for b in lib.books().unwrap_or_default() {
            counts.insert(Nav::Annotations(View::Book(b.id)), b.annotation_count);
        }
        for t in lib.tags().unwrap_or_default() {
            counts.insert(Nav::Annotations(View::Tag(t.id)), t.count);
        }
        for (nav, label) in self.count_labels.borrow().iter() {
            let n = counts.get(nav).copied().unwrap_or(0);
            label.set_label(&if n > 0 { n.to_string() } else { String::new() });
            // "Deleted on Kobo" only shows up when there's something in it
            // (or while you're looking at it).
            if *nav == Nav::Annotations(View::RemovedOnDevice)
                && let Some(row) = label.ancestor(gtk::ListBoxRow::static_type())
            {
                row.set_visible(n > 0 || self.current.get() == *nav);
            }
        }
    }

    pub(super) fn select_nav(&self, nav: Nav) {
        let index = self
            .sidebar_navs
            .borrow()
            .iter()
            .position(|n| *n == Some(nav));
        if let Some(i) = index {
            self.sidebar
                .select_row(self.sidebar.row_at_index(i as i32).as_ref());
        }
    }
}
