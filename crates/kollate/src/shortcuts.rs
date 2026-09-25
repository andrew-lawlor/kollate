//! The Keyboard Shortcuts window (Ctrl+?). libadwaita's own shortcuts
//! dialog needs 1.8; this builds the same kind of list with 1.7.

use adw::prelude::*;

/// (group title, [(keys, description)]). In `keys`, alternatives are
/// separated by " / " and keys pressed together by "+".
const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Triage",
        &[
            ("K", "Keep"),
            ("A", "Archive"),
            ("S", "Star or unstar (starring in the Inbox also keeps)"),
            ("I", "Move back to the Inbox"),
            ("Delete", "Move to Trash"),
            ("E / Enter", "Edit the highlight, note and tags"),
        ],
    ),
    (
        "Selecting Several",
        &[
            ("Ctrl+Click", "Add or remove a highlight"),
            ("Shift+Click", "Select a range"),
            ("Shift+↑ / Shift+↓", "Extend the selection"),
            ("Ctrl+A", "Select everything listed"),
            ("Escape", "Back to one selected highlight"),
        ],
    ),
    (
        "Moving Around",
        &[
            ("↑ / ↓", "Previous or next highlight"),
            ("Alt+←", "Back to Books"),
            ("Ctrl+F", "Search"),
            ("Escape", "Clear the search"),
        ],
    ),
    (
        "General",
        &[
            ("Ctrl+I", "Import from your Kobo"),
            ("Ctrl+E", "Export"),
            ("Ctrl+,", "Preferences"),
            ("Ctrl+?", "Keyboard shortcuts"),
            ("Ctrl+W", "Close the window"),
            ("Ctrl+Q", "Quit"),
        ],
    ),
];

/// Keys drawn as keycaps: `Ctrl+A / Enter` → [Ctrl]+[A] or [Enter].
fn keycaps(keys: &str) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(4)
        .valign(gtk::Align::Center)
        .build();
    for (i, alternative) in keys.split(" / ").enumerate() {
        if i > 0 {
            row.append(
                &gtk::Label::builder()
                    .label("or")
                    .css_classes(["dim-label", "caption"])
                    .margin_start(4)
                    .margin_end(4)
                    .build(),
            );
        }
        for (j, key) in alternative.split('+').enumerate() {
            if j > 0 {
                row.append(
                    &gtk::Label::builder()
                        .label("+")
                        .css_classes(["dim-label"])
                        .build(),
                );
            }
            row.append(
                &gtk::Label::builder()
                    .label(key)
                    .css_classes(["keycap"])
                    .build(),
            );
        }
    }
    row
}

pub fn present(parent: &impl IsA<gtk::Widget>) {
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .build();
    for (title, entries) in GROUPS {
        let group = adw::PreferencesGroup::builder().title(*title).build();
        if *title == "Selecting Several" {
            group.set_description(Some(
                "With several highlights selected, the triage keys act on all of them.",
            ));
        }
        for (keys, description) in *entries {
            let row = adw::ActionRow::builder().title(*description).build();
            row.add_suffix(&keycaps(keys));
            group.add(&row);
        }
        body.append(&group);
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&adw::Clamp::builder().maximum_size(560).child(&body).build())
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(560)
        .content_height(640)
        .child(&toolbar)
        .build();
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_shortcut_has_keys_and_a_description() {
        for (_, entries) in super::GROUPS {
            for (keys, description) in *entries {
                assert!(!keys.trim().is_empty() && !description.trim().is_empty());
                assert!(
                    keys.split(" / ")
                        .all(|alt| alt.split('+').all(|k| !k.is_empty())),
                    "{keys}"
                );
            }
        }
    }
}
