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
use kollate_core::kobo::{
    DeviceInfo, KoboDb, find_kobo_db, find_mounted_kobos, is_kobo_mount, is_tested_db_version,
};
use kollate_core::store::{
    Annotation, AnnotationFilter, Book, BookSort, DeviceDeletePolicy, Status, View, Vocab,
    VocabStatus,
};
use kollate_core::{ImportStats, Library};

use crate::{card, edit, portal, shortcuts, word};

mod annotations;
mod content;
mod device;
mod exports;
mod preferences;
mod sidebar;

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
}
