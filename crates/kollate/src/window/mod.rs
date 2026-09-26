//! Main window: a sidebar of views, books and tags, and a list of cards.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use kollate_core::dict::{self, Dictionary};
use kollate_core::export::{self, ExportOptions};
use kollate_core::kobo::assets::{CopiedAssets, copy_assets};
use kollate_core::kobo::epub::find_word_contexts;
use kollate_core::kobo::{DeviceInfo, KoboDb, find_kobo_db, find_mounted_kobos, is_kobo_mount};
use kollate_core::store::{
    Annotation, AnnotationFilter, Book, BookSort, DeviceDeletePolicy, Status, View, Vocab,
    VocabStatus,
};
use kollate_core::{ImportStats, Library};

use crate::{card, edit, portal, shortcuts, word};

/// What the content pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nav {
    Annotations(View),
    Vocab,
    /// The Books page (cover grid).
    Books,
}

const TOP_LEVEL: [(Nav, &str, &str); 10] = [
    (
        Nav::Annotations(View::Inbox),
        "mail-unread-symbolic",
        "Inbox",
    ),
    (
        Nav::Annotations(View::All),
        "view-list-bullet-symbolic",
        "All Highlights",
    ),
    (
        Nav::Annotations(View::Notes),
        "document-edit-symbolic",
        "Notes",
    ),
    (
        Nav::Annotations(View::Markups),
        "input-tablet-symbolic",
        "Markups",
    ),
    (
        Nav::Annotations(View::Starred),
        "starred-symbolic",
        "Starred",
    ),
    (Nav::Books, "folder-documents-symbolic", "Books"),
    (Nav::Vocab, "accessories-dictionary-symbolic", "Vocabulary"),
    (
        Nav::Annotations(View::Archive),
        "folder-symbolic",
        "Archive",
    ),
    (
        Nav::Annotations(View::Trash),
        "user-trash-symbolic",
        "Trash",
    ),
    (
        Nav::Annotations(View::RemovedOnDevice),
        "edit-delete-symbolic",
        "Deleted on Kobo",
    ),
];

/// What to do when a Kobo is plugged in (setting `on_connect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConnect {
    Import,
    Ask,
    Nothing,
}

impl OnConnect {
    pub const ALL: [Self; 3] = [Self::Import, Self::Ask, Self::Nothing];
    pub const LABELS: [&str; 3] = ["Import automatically", "Ask first", "Do nothing"];

    fn key(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Ask => "ask",
            Self::Nothing => "nothing",
        }
    }

    fn from_key(key: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|v| Some(v.key()) == key)
            .unwrap_or(Self::Import)
    }
}

/// A Kobo that is currently mounted.
struct Connected {
    mount: gio::Mount,
    root: PathBuf,
    name: &'static str,
}

/// What the banner's button does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BannerAction {
    Import,
    Eject,
}

/// Credit line for dictionaries converted from reader.dict's Wiktionary extracts.
const WIKTIONARY_SOURCE: &str = "Wiktionary contributors, via reader.dict (CC BY-SA 4.0)";

/// Where to get Wiktionary dictionaries for other languages.
const MORE_DICTIONARIES_URL: &str = "https://www.reader-dict.com/";

/// How many books the sidebar lists under Recent.
const RECENT_BOOKS: usize = 5;

const VOCAB_STATUS_LABELS: [&str; 4] = ["New", "Learning", "Known", "Ignored"];

pub struct Window {
    win: adw::ApplicationWindow,
    lib: RefCell<Library>,
    split: adw::NavigationSplitView,
    toasts: adw::ToastOverlay,
    /// The most recent toast; replaced rather than queued behind.
    last_toast: RefCell<Option<adw::Toast>>,
    /// The Preferences or Export dialog, while open. Toasts go there,
    /// because a dialog covers the window's own toasts.
    open_dialog: RefCell<Option<glib::WeakRef<adw::PreferencesDialog>>>,

    sidebar: gtk::ListBox,
    /// Parallel to the sidebar's rows; `None` for section headers.
    sidebar_navs: RefCell<Vec<Option<Nav>>>,
    count_labels: RefCell<HashMap<Nav, gtk::Label>>,
    books: RefCell<HashMap<i64, (String, Option<String>)>>,
    tags: RefCell<HashMap<i64, String>>,
    rebuilding_sidebar: Cell<bool>,

    content_page: adw::NavigationPage,
    title: adw::WindowTitle,
    banner: adw::Banner,
    tip: adw::Banner,
    banner_action: Cell<BannerAction>,
    monitor: gio::VolumeMonitor,
    kobo: RefCell<Option<Connected>>,
    importing: Cell<bool>,
    selection_bar: gtk::ActionBar,
    selection_label: gtk::Label,
    selection_done: gtk::Button,
    keep_all: gtk::Button,
    search: gtk::SearchEntry,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    list: gtk::ListBox,
    /// Parallel to the list's rows: the group heading each row belongs to.
    groups: Rc<RefCell<Vec<String>>>,
    /// Number of (annotation, vocab) rows currently listed.
    shown: Cell<(usize, usize)>,
    /// Books page.
    books_grid: gtk::FlowBox,
    books_shown: Cell<usize>,
    book_sort: gtk::DropDown,
    back: gtk::Button,
    /// The current book was opened from the Books page (Back returns there).
    from_books: Cell<bool>,
    /// Book IDs listed under Recent in the sidebar.
    recent_ids: RefCell<Vec<i64>>,
    current: Cell<Nav>,
    dictionaries: RefCell<Vec<Dictionary>>,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Window {
    pub fn new(app: &adw::Application, lib: Library) -> Rc<Self> {
        let win = adw::ApplicationWindow::builder()
            .application(app)
            .title("Kollate")
            .default_width(1100)
            .default_height(760)
            .width_request(360)
            .height_request(400)
            .build();

        // Sidebar
        let menu = gio::Menu::new();
        let import = gio::Menu::new();
        import.append(Some("_Import from Kobo"), Some("win.import"));
        import.append(Some("Import from _Folder…"), Some("win.import-folder"));
        menu.append_section(None, &import);
        let export = gio::Menu::new();
        export.append(Some("_Export…"), Some("win.export"));
        menu.append_section(None, &export);
        let help = gio::Menu::new();
        help.append(Some("_Preferences"), Some("win.preferences"));
        help.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
        help.append(Some("_About Kollate"), Some("win.about"));
        menu.append_section(None, &help);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        let import_button = gtk::Button::builder()
            .icon_name("media-removable-symbolic")
            .tooltip_text("Import from Kobo (Ctrl+I)")
            .action_name("win.import")
            .build();
        import_button.update_property(&[gtk::accessible::Property::Label("Import from Kobo")]);
        let export_button = gtk::Button::builder()
            .icon_name("send-to-symbolic")
            .tooltip_text("Export (Ctrl+E)")
            .action_name("win.export")
            .build();
        export_button.update_property(&[gtk::accessible::Property::Label("Export")]);
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.pack_start(&import_button);
        sidebar_header.pack_start(&export_button);
        sidebar_header.pack_end(&menu_button);
        let sidebar = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let sidebar_toolbar = adw::ToolbarView::new();
        sidebar_toolbar.add_top_bar(&sidebar_header);
        sidebar_toolbar.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&sidebar)
                .build(),
        ));
        let sidebar_page = adw::NavigationPage::new(&sidebar_toolbar, "Kollate");

        // Content
        let title = adw::WindowTitle::new("", "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let keep_all = gtk::Button::builder()
            .label("Keep All")
            .tooltip_text("Keep every highlight listed in the Inbox")
            .visible(false)
            .build();
        header.pack_end(&keep_all);
        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back to Books (Alt+←)")
            .action_name("win.back")
            .visible(false)
            .build();
        back.update_property(&[gtk::accessible::Property::Label("Back to Books")]);
        header.pack_start(&back);
        let book_sort = gtk::DropDown::from_strings(&["Recent", "Title", "Author"]);
        book_sort.set_tooltip_text(Some("Sort books"));
        book_sort.update_property(&[gtk::accessible::Property::Label("Sort books")]);
        book_sort.set_visible(false);
        header.pack_end(&book_sort);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search text, notes, books and tags")
            .hexpand(true)
            .build();
        search.set_tooltip_text(Some("Search (Ctrl+F)"));
        // Always visible, so it's easy to find; Escape clears it.
        let search_bar = adw::Clamp::builder()
            .maximum_size(600)
            .child(&search)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        search.connect_stop_search(|entry| entry.set_text(""));

        // Shown while several highlights are selected.
        let selection_label = gtk::Label::new(None);
        let selection_bar = gtk::ActionBar::builder().revealed(false).build();
        selection_bar.set_center_widget(Some(&selection_label));
        for (label, tooltip, action) in [
            ("Keep", "Keep (K)", "keep"),
            ("Archive", "Archive (A)", "archive"),
            ("Star", "Star or unstar (S)", "star"),
        ] {
            let b = gtk::Button::builder()
                .label(label)
                .tooltip_text(tooltip)
                .action_name("win.bulk")
                .action_target(&action.to_variant())
                .build();
            // A text button is named by its label; replace that with a fuller name.
            b.reset_relation(gtk::AccessibleRelation::LabelledBy);
            b.update_property(&[gtk::accessible::Property::Label(&format!(
                "{label} selected highlights"
            ))]);
            selection_bar.pack_start(&b);
        }
        let bulk_trash = gtk::Button::builder()
            .label("Trash")
            .tooltip_text("Move to Trash (Delete)")
            .action_name("win.bulk")
            .action_target(&"trash".to_variant())
            .css_classes(["destructive-action"])
            .build();
        bulk_trash.reset_relation(gtk::AccessibleRelation::LabelledBy);
        bulk_trash.update_property(&[gtk::accessible::Property::Label(
            "Trash selected highlights",
        )]);
        selection_bar.pack_start(&bulk_trash);
        let clear_selection = gtk::Button::builder()
            .label("Done")
            .tooltip_text("Clear the selection (Escape)")
            .build();
        selection_bar.pack_end(&clear_selection);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Multiple)
            .css_classes(["boxed-list-separate", "annotation-list"])
            .valign(gtk::Align::Start)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(820)
            .tightening_threshold(600)
            .child(&list)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let empty = adw::StatusPage::new();
        let books_grid = gtk::FlowBox::builder()
            .homogeneous(true)
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .min_children_per_line(2)
            .max_children_per_line(10)
            .column_spacing(12)
            .row_spacing(12)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        let books_scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(1100)
                    .child(&books_grid)
                    .build(),
            )
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(&scrolled, Some("list"));
        stack.add_named(&books_scrolled, Some("books"));
        stack.add_named(&empty, Some("empty"));
        let banner = adw::Banner::new("");
        // One-time hint about keyboard triage, shown in the Inbox.
        let tip = adw::Banner::builder()
            .title("Tip: K keeps, A archives, S stars, and Delete trashes the selected highlight. Press Ctrl+? for all shortcuts.")
            .button_label("Got It")
            .build();
        let content_toolbar = adw::ToolbarView::new();
        content_toolbar.add_top_bar(&header);
        content_toolbar.add_top_bar(&banner);
        content_toolbar.add_top_bar(&tip);
        content_toolbar.add_top_bar(&search_bar);
        content_toolbar.add_bottom_bar(&selection_bar);
        content_toolbar.set_content(Some(&stack));
        let content_page = adw::NavigationPage::new(&content_toolbar, "Inbox");

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content_page)
            .min_sidebar_width(220.0)
            .max_sidebar_width(300.0)
            .build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        win.set_content(Some(&toasts));

        let breakpoint = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 640sp").expect("valid"),
        );
        breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
        win.add_breakpoint(breakpoint);

        let this = Rc::new(Self {
            win,
            lib: RefCell::new(lib),
            split,
            toasts,
            last_toast: RefCell::default(),
            open_dialog: RefCell::default(),
            sidebar,
            sidebar_navs: RefCell::default(),
            count_labels: RefCell::default(),
            books: RefCell::default(),
            tags: RefCell::default(),
            rebuilding_sidebar: Cell::new(false),
            content_page,
            title,
            banner,
            tip,
            banner_action: Cell::new(BannerAction::Import),
            monitor: gio::VolumeMonitor::get(),
            kobo: RefCell::default(),
            importing: Cell::new(false),
            selection_bar,
            selection_label,
            selection_done: clear_selection,
            keep_all,
            search,
            stack,
            empty,
            list,
            groups: Rc::default(),
            shown: Cell::new((0, 0)),
            books_grid,
            books_shown: Cell::new(0),
            book_sort,
            back,
            from_books: Cell::new(false),
            recent_ids: RefCell::default(),
            current: Cell::new(Nav::Annotations(View::Inbox)),
            dictionaries: RefCell::default(),
        });
        this.setup_list();
        this.setup_signals();
        this.setup_actions();
        this.load_dictionaries();
        this.enrich_vocab();
        this.rebuild_sidebar();
        this.reload();
        this.setup_device_monitor();

        // The window owns the controller for as long as it's open.
        let keep_alive = this.clone();
        this.win.connect_close_request(move |_| {
            let _ = &keep_alive;
            glib::Propagation::Proceed
        });
        this
    }

    pub fn present(&self) {
        self.win.present();
    }

    fn toast(&self, title: &str) {
        self.show_toast(adw::Toast::new(title));
    }

    fn show_toast(&self, toast: adw::Toast) {
        if let Some(previous) = self.last_toast.replace(Some(toast.clone())) {
            previous.dismiss();
        }
        let dialog = self.open_dialog.borrow().as_ref().and_then(|d| d.upgrade());
        match dialog {
            Some(dialog) => dialog.add_toast(toast),
            None => self.toasts.add_toast(toast),
        }
    }

    /// Sends toasts to `dialog` until it closes.
    fn track_dialog(self: &Rc<Self>, dialog: &adw::PreferencesDialog) {
        *self.open_dialog.borrow_mut() = Some(dialog.downgrade());
        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |closed| {
            let Some(this) = weak.upgrade() else { return };
            let mut open = this.open_dialog.borrow_mut();
            if open.as_ref().and_then(|d| d.upgrade()).as_ref() == Some(closed) {
                *open = None;
            }
        });
    }

    fn error(&self, heading: &str, err: impl std::fmt::Display) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(&err.to_string()));
        dialog.add_response("close", "Close");
        dialog.present(Some(&self.win));
    }

    // ---- Setup -------------------------------------------------------------

    fn setup_list(self: &Rc<Self>) {
        let groups = self.groups.clone();
        self.list.set_header_func(move |row, before| {
            let groups = groups.borrow();
            let label = groups
                .get(row.index() as usize)
                .cloned()
                .unwrap_or_default();
            let previous = before.and_then(|b| groups.get(b.index() as usize));
            if label.is_empty() || previous == Some(&label) {
                row.set_header(None::<&gtk::Widget>);
                return;
            }
            let header = gtk::Label::builder()
                .label(&label)
                .xalign(0.0)
                .wrap(true)
                .css_classes(["heading", "group-header"])
                .build();
            if before.is_none() {
                header.add_css_class("first");
            }
            row.set_header(Some(&header));
        });

        // Single-key triage on the selected card(s). With several selected,
        // K/A/S/I/Delete apply to all of them.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if state.contains(gdk::ModifierType::CONTROL_MASK)
                && matches!(key, gdk::Key::a | gdk::Key::A)
            {
                this.list.select_all();
                return glib::Propagation::Stop;
            }
            if state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let action = match key {
                gdk::Key::k => "keep",
                gdk::Key::a => "archive",
                gdk::Key::s => "star",
                gdk::Key::e => "edit",
                gdk::Key::i => "inbox",
                gdk::Key::Delete | gdk::Key::KP_Delete => "trash",
                gdk::Key::Escape if this.list.selected_rows().len() > 1 => {
                    this.collapse_selection();
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            let ids = this.selected_annotation_ids();
            if ids.len() > 1 && action != "edit" {
                this.bulk(action, &ids);
                return glib::Propagation::Stop;
            }
            let row = this.list.selected_rows().into_iter().next();
            match row {
                Some(row) if row.activate_action(&format!("card.{action}"), None).is_ok() => {
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.list.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.list.connect_selected_rows_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.update_selection_bar();
            }
        });

        // Double-click or Enter on a card opens the editor.
        self.list.set_activate_on_single_click(false);
        self.list.connect_row_activated(|_, row| {
            let _ = row.activate_action("card.edit", None);
        });
    }

    fn setup_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.sidebar.connect_row_selected(move |_, row| {
            let (Some(this), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            if this.rebuilding_sidebar.get() {
                return;
            }
            let nav = this
                .sidebar_navs
                .borrow()
                .get(row.index() as usize)
                .copied()
                .flatten();
            if let Some(nav) = nav {
                this.navigate(nav, false);
            }
        });

        // Clicking the (already selected) Books row while a book from the
        // Books page is open goes back to the grid.
        let weak = Rc::downgrade(self);
        self.sidebar.connect_row_activated(move |_, row| {
            let Some(this) = weak.upgrade() else { return };
            let nav = this
                .sidebar_navs
                .borrow()
                .get(row.index() as usize)
                .copied()
                .flatten();
            if nav == Some(Nav::Books) && this.current.get() != Nav::Books {
                this.navigate(Nav::Books, false);
            }
        });

        let saved = BookSort::parse(
            self.lib
                .borrow()
                .setting("books_sort")
                .ok()
                .flatten()
                .as_deref(),
        );
        self.book_sort
            .set_selected(BookSort::ALL.iter().position(|s| *s == saved).unwrap_or(0) as u32);
        let weak = Rc::downgrade(self);
        self.book_sort.connect_selected_notify(move |dd| {
            let Some(this) = weak.upgrade() else { return };
            let sort = BookSort::ALL[(dd.selected() as usize).min(BookSort::ALL.len() - 1)];
            if let Err(err) = this.lib.borrow().set_setting("books_sort", sort.as_str()) {
                this.error("Couldn’t Save Setting", err);
            }
            if this.current.get() == Nav::Books {
                this.reload();
            }
        });

        let weak = Rc::downgrade(self);
        self.tip.connect_button_clicked(move |tip| {
            tip.set_revealed(false);
            if let Some(this) = weak.upgrade()
                && let Err(err) = this.lib.borrow().set_setting("tip_triage_dismissed", "1")
            {
                this.error("Couldn’t Save Setting", err);
            }
        });

        let weak = Rc::downgrade(self);
        self.keep_all.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                let ids = this.listed_annotation_ids();
                this.bulk("keep", &ids);
            }
        });
        let weak = Rc::downgrade(self);
        self.selection_done.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.collapse_selection();
            }
        });

        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.reload();
            }
        });
    }

    /// Shows `nav`. `from_books` records that a book was opened from the
    /// Books page, so Back can return there.
    fn navigate(self: &Rc<Self>, nav: Nav, from_books: bool) {
        self.current.set(nav);
        self.from_books.set(from_books);
        if !self.search.text().is_empty() {
            self.search.set_text(""); // triggers a reload
        } else {
            self.reload();
        }
        self.split.set_show_content(true);
        self.update_counts();
    }

    fn add_win_action(self: &Rc<Self>, name: &str, f: impl Fn(&Rc<Self>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, _| {
            if let Some(this) = weak.upgrade() {
                f(&this);
            }
        });
        self.win.add_action(&action);
    }

    fn setup_actions(self: &Rc<Self>) {
        self.add_win_action("search", |this| {
            this.search.grab_focus();
            this.search.select_region(0, -1);
        });

        let bulk = gio::SimpleAction::new("bulk", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        bulk.connect_activate(move |_, arg| {
            if let (Some(this), Some(action)) = (weak.upgrade(), arg.and_then(|a| a.str())) {
                let ids = this.selected_annotation_ids();
                this.bulk(action, &ids);
            }
        });
        self.win.add_action(&bulk);
        self.add_win_action("import", |this| {
            let connected = this.kobo.borrow().as_ref().map(|k| k.root.clone());
            match connected.or_else(|| find_mounted_kobos().into_iter().next()) {
                Some(mount) => this.import_from(mount),
                None => this.choose_import_folder(),
            }
        });
        self.add_win_action("preferences", |this| this.show_preferences());
        self.add_win_action("export", |this| this.show_export());
        self.add_win_action("back", |this| {
            if this.from_books.get() {
                this.navigate(Nav::Books, false);
            }
        });
        self.add_win_action("import-folder", |this| this.choose_import_folder());
        self.add_win_action("about", |this| {
            let about = adw::AboutDialog::builder()
                .application_name("Kollate")
                .application_icon(crate::APP_ID)
                .developer_name("Andrew Lawlor")
                .website("https://github.com/andrew-lawlor/kollate")
                .version(env!("CARGO_PKG_VERSION"))
                .comments("Collect and curate highlights, notes and vocabulary from your Kobo.")
                .license_type(gtk::License::Gpl30)
                .build();
            about.add_legal_section(
                "English Wiktionary",
                Some("© Wiktionary contributors"),
                gtk::License::Custom,
                Some("Definitions from <a href=\"https://www.wiktionary.org/\">Wiktionary</a>, compiled by <a href=\"https://www.reader-dict.com/\">reader.dict</a>, licensed under <a href=\"https://creativecommons.org/licenses/by-sa/4.0/\">CC BY-SA 4.0</a>."),
            );
            about.present(Some(&this.win));
        });
        self.add_win_action("shortcuts", |this| shortcuts::present(&this.win));
    }

    // ---- Sidebar -----------------------------------------------------------

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

    fn rebuild_sidebar(self: &Rc<Self>) {
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

    fn update_counts(self: &Rc<Self>) {
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

    fn select_nav(&self, nav: Nav) {
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

    // ---- Content -----------------------------------------------------------

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

    fn update_title(&self) {
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

    fn search_text(&self) -> Option<String> {
        Some(self.search.text().to_string()).filter(|s| !s.trim().is_empty())
    }

    /// Reloads the content pane for the current view.
    fn reload(self: &Rc<Self>) {
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

    fn update_empty_state(&self) {
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
    fn load_dictionaries(&self) {
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
    fn enrich_vocab(&self) -> usize {
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

    // ---- Annotation rows and actions ----------------------------------------

    fn annotation_row(self: &Rc<Self>, a: &Annotation, in_book_view: bool) -> gtk::ListBoxRow {
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
    fn selected_annotation_ids(&self) -> Vec<i64> {
        let mut rows = self.list.selected_rows();
        rows.sort_by_key(|r| r.index());
        rows.iter()
            .filter_map(|r| r.widget_name().strip_prefix('a')?.parse().ok())
            .collect()
    }

    /// IDs of every highlight row currently listed.
    fn listed_annotation_ids(&self) -> Vec<i64> {
        (0..)
            .map_while(|i| self.list.row_at_index(i))
            .filter_map(|r| r.widget_name().strip_prefix('a')?.parse().ok())
            .collect()
    }

    /// Keeps only the first selected row selected.
    fn collapse_selection(&self) {
        let mut rows = self.list.selected_rows();
        rows.sort_by_key(|r| r.index());
        if let Some(first) = rows.first() {
            self.list.unselect_all();
            self.list.select_row(Some(first));
            first.grab_focus();
        }
    }

    fn update_selection_bar(&self) {
        let n = self.selected_annotation_ids().len();
        let show = n > 1 && matches!(self.current.get(), Nav::Annotations(_));
        self.selection_bar.set_revealed(show);
        if show {
            self.selection_label.set_label(&format!("{n} selected"));
        }
    }

    /// Applies `action` (keep, archive, trash, inbox or star) to several
    /// highlights at once, with one Undo for all of them.
    fn bulk(self: &Rc<Self>, action: &str, ids: &[i64]) {
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

    fn edit(self: &Rc<Self>, row: &gtk::ListBoxRow, id: i64) {
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

    // ---- Import ------------------------------------------------------------

    fn choose_import_folder(self: &Rc<Self>) {
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
    fn import_from(self: &Rc<Self>, path: PathBuf) {
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
                            Ok((stats, assets.err()))
                        })
                        .map_err(|e| e.to_string())
                }
                Ok(Err(err)) => Err(err.to_string()),
                Err(_) => Err("The import stopped unexpectedly.".to_owned()),
            };
            match outcome {
                Ok((stats, asset_error)) => {
                    this.enrich_vocab();
                    this.rebuild_sidebar();
                    this.reload();
                    this.import_toast(&stats);
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

    fn on_connect_setting(&self) -> OnConnect {
        OnConnect::from_key(
            self.lib
                .borrow()
                .setting("on_connect")
                .ok()
                .flatten()
                .as_deref(),
        )
    }

    fn setup_device_monitor(self: &Rc<Self>) {
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

    fn show_preferences(self: &Rc<Self>) {
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

    // ---- Export -------------------------------------------------------------

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

    fn auto_sync_obsidian(&self) {
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

    fn show_export(self: &Rc<Self>) {
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
