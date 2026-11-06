//! The native menu bar (`Services::native_menu`): a `BMenuBar` at the top of the window instead of
//! the in-window menus. What the menus hold, and when they change, is upstream's
//! (`vectorcraft_ui_egui::native_menu`); this file lays them out the Haiku way and keeps the
//! `BMenuBar` in step (`vc_menu_build`, `vc_menu_update` in host.cpp).
//!
//! - **Layout.** Upstream hands over a Mac menu bar: an app menu named VectorCraft holding About,
//!   Settings, Language, Appearance, Hide and Quit. A Haiku app has no app menu, so they go back
//!   where Haiku apps keep them: About at the end of Help, Settings (Edit › Preferences' pages) at
//!   the end of Edit, Language and Appearance at the end of Window, Quit at the end of File.
//!   AppKit's own items (Services, Hide, Hide Others, Show All, Bring All to Front) are dropped;
//!   Minimize and Zoom are the window's own. Help › Check for Updates goes too: SoftwareUpdater
//!   updates the package.
//! - **Keys stay with the interface**, as upstream's contract asks: the window hands every
//!   Command key to the canvas view before `BWindow` looks for a menu shortcut (`DispatchMessage`
//!   in host.cpp), so the menus only show the shortcuts. Those with Command (`Cmd+…`) and one
//!   character are drawn by `BMenuItem` itself; the others (a function key, Page Up, a tool's bare
//!   letter) as text, without becoming window shortcuts. Command+Q still reaches `BWindow`,
//!   which runs the item showing it (File › Quit).
//! - A structure change rebuilds the whole bar (fast enough with a few hundred items); labels,
//!   enabled and checked are updated in place, only for the items that changed.
//! - `VECTORCRAFT_NATIVE_MENU=0` keeps the in-window menus.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;

use vectorcraft_ui_egui::native_menu::{self, Backend, Event, Item, Menu, MenuBar, MenuRole, Node, Standard};

/// `B_SHIFT_KEY`, `B_COMMAND_KEY`, `B_CONTROL_KEY`, `B_OPTION_KEY` (`<InterfaceDefs.h>`).
const SHIFT: u32 = 0x1;
const COMMAND: u32 = 0x2;
const CONTROL: u32 = 0x4;
const OPTION: u32 = 0x40;

/// Commands the Haiku bar leaves out (see the module docs).
const LEFT_OUT: &[&str] = &[native_menu::HIDE, native_menu::HIDE_OTHERS, "help.checkForUpdates"];
/// The app menu's items that go back to the menus Haiku apps keep them in.
const ABOUT: &str = "help.about";
const QUIT: &str = "app.quit";

/// Where the bar goes: the host in the app, a recorder in tests.
pub trait Sink {
    fn build(&mut self, spec: &str);
    fn update(&mut self, spec: &str);
}

/// The window's menu bar.
#[cfg(target_os = "haiku")]
pub struct HostSink;

#[cfg(target_os = "haiku")]
impl Sink for HostSink {
    fn build(&mut self, spec: &str) {
        crate::host::menu_build(spec);
    }

    fn update(&mut self, spec: &str) {
        crate::host::menu_update(spec);
    }
}

/// Clicks reported by the window ([`crate::host::kind::MENU`]), by item index.
pub type Clicks = Rc<RefCell<Vec<i32>>>;

pub struct HaikuMenu<S: Sink> {
    sink: S,
    clicks: Clicks,
    /// Upstream's structure key of the bar last built.
    built: Option<u64>,
    /// The bar's items by index, as last sent.
    entries: Vec<Item>,
}

impl<S: Sink> HaikuMenu<S> {
    pub fn new(sink: S, clicks: Clicks) -> Self {
        Self { sink, clicks, built: None, entries: Vec::new() }
    }
}

/// Two items are the same entry of the bar when they run the same command with the same params.
fn same_entry(a: &Item, b: &Item) -> bool {
    a.command == b.command && a.params == b.params && a.source == b.source
}

impl<S: Sink> Backend for HaikuMenu<S> {
    fn sync(&mut self, bar: &MenuBar) {
        let key = native_menu::structure_key(bar);
        let bar = haiku_layout(bar);
        if self.built != Some(key) {
            let (spec, entries) = spec(&bar);
            self.sink.build(&spec);
            self.entries = entries;
            self.built = Some(key);
            // Clicks on the old bar's items no longer mean anything.
            self.clicks.borrow_mut().clear();
            return;
        }
        let mut changes = String::new();
        for (i, (e, it)) in self.entries.iter_mut().zip(bar.items()).enumerate() {
            if !same_entry(e, it) {
                continue;
            }
            if e.label != it.label || e.enabled != it.enabled || e.checked != it.checked {
                let _ = writeln!(changes, "{i}\t{}\t{}\t{}", field(&it.label), i32::from(it.enabled), checked(it.checked));
                e.clone_from(it);
            }
        }
        if !changes.is_empty() {
            self.sink.update(&changes);
        }
    }

    fn drain(&mut self) -> Vec<Event> {
        let clicks = std::mem::take(&mut *self.clicks.borrow_mut());
        clicks.into_iter().filter_map(|i| usize::try_from(i).ok().and_then(|i| self.entries.get(i))).map(|e| Event::Click(e.clone())).collect()
    }
}

fn runs(n: &Node, command: &str) -> bool {
    matches!(n, Node::Item(it) if it.command == Some(command))
}

/// Upstream's Mac menu bar laid out the Haiku way (see the module docs).
pub fn haiku_layout(bar: &MenuBar) -> MenuBar {
    let mut menus = bar.menus.clone();
    if let Some(app) = menus.iter().position(|m| m.role == MenuRole::App).map(|i| menus.remove(i)) {
        let (mut about, mut settings, mut window, mut quit) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        // The app menu's submenus are Settings, then Language and Appearance (`mac_layout`).
        let mut submenus = 0;
        for n in app.children {
            match &n {
                _ if runs(&n, ABOUT) => about.push(n),
                Node::Item(it) if it.command == Some(QUIT) => {
                    // "Quit VectorCraft", as Haiku apps call it too.
                    quit.push(n);
                }
                Node::Submenu { .. } => {
                    if submenus == 0 {
                        settings.push(n);
                    } else {
                        window.push(n);
                    }
                    submenus += 1;
                }
                _ => {}
            }
        }
        // The titles are translated: File and Edit are the first two plain menus.
        let normal: Vec<usize> = menus.iter().enumerate().filter(|(_, m)| m.role == MenuRole::Normal).map(|(i, _)| i).collect();
        let role = |r: MenuRole| menus.iter().position(|m| m.role == r);
        let places = [(normal.get(1).copied(), settings), (role(MenuRole::Window), window), (role(MenuRole::Help), about), (normal.first().copied(), quit)];
        for (at, nodes) in places {
            if let Some(m) = at.and_then(|i| menus.get_mut(i))
                && !nodes.is_empty()
            {
                m.children.push(Node::Separator);
                m.children.extend(nodes);
            }
        }
    }
    for m in &mut menus {
        m.children = tidy(haiku_nodes(std::mem::take(&mut m.children)));
    }
    menus.retain(|m| !m.children.is_empty());
    MenuBar { menus }
}

/// AppKit's own items go; Zoom stays (the window's own).
fn haiku_nodes(nodes: Vec<Node>) -> Vec<Node> {
    nodes
        .into_iter()
        .filter_map(|n| match n {
            Node::Standard(Standard::Services | Standard::ShowAll | Standard::BringAllToFront) => None,
            Node::Item(it) if it.command.is_some_and(|c| LEFT_OUT.contains(&c)) => None,
            Node::Submenu { label, children } => Some(Node::Submenu { label, children: tidy(haiku_nodes(children)) }),
            other => Some(other),
        })
        .collect()
}

/// No separators at either end of a menu, or two in a row.
fn tidy(nodes: Vec<Node>) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if matches!(n, Node::Separator) && matches!(out.last(), None | Some(Node::Separator)) {
            continue;
        }
        out.push(n);
    }
    while matches!(out.last(), Some(Node::Separator)) {
        out.pop();
    }
    out
}

/// Text for one field of a spec line.
fn field(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn checked(c: Option<bool>) -> i32 {
    c.map_or(-1, i32::from)
}

/// A shortcut as the menu shows it (`Cmd+Shift+Z`, as the registry writes it): a character and
/// modifiers `BMenuItem` draws (Command shortcuts with one character), or text.
pub fn shortcut(s: &str) -> (u8, u32, String) {
    let parts: Vec<&str> = s.split('+').collect();
    let Some((key, mods)) = parts.split_last() else { return (0, 0, String::new()) };
    // A shortcut whose key is the plus sign ("Cmd++") splits into an empty last part.
    let key = if key.is_empty() && s.ends_with("++") { "+" } else { key };
    let has = |m: &str| mods.iter().any(|x| x.eq_ignore_ascii_case(m));
    if has("Cmd") {
        let ch = match key {
            k if k.len() == 1 && k.is_ascii() && !k.chars().all(char::is_whitespace) => k.as_bytes()[0].to_ascii_uppercase(),
            "Delete" => 0x7f,
            "Backspace" => 0x08,
            "Enter" => 0x0a,
            "Tab" => 0x09,
            "Space" => 0x20,
            _ => 0,
        };
        if ch != 0 {
            let mut bits = COMMAND;
            for (on, bit) in [(has("Ctrl"), CONTROL), (has("Alt"), OPTION), (has("Shift"), SHIFT)] {
                if on {
                    bits |= bit;
                }
            }
            return (ch, bits, String::new());
        }
    }
    (0, 0, s.to_string())
}

/// The host's spec of `bar` (`BuildMenuBar` in host.cpp), and its items by index.
pub fn spec(bar: &MenuBar) -> (String, Vec<Item>) {
    fn walk(out: &mut String, entries: &mut Vec<Item>, nodes: &[Node]) {
        for n in nodes {
            match n {
                Node::Separator => out.push_str("-\n"),
                Node::Header(label) => {
                    let _ = writeln!(out, "I\t-1\t{}\t0\t0\t\t0\t-1", field(label));
                }
                Node::Submenu { label, children } => {
                    let _ = writeln!(out, "S\t{}", field(label));
                    walk(out, entries, children);
                    out.push_str("E\n");
                }
                Node::Standard(Standard::Zoom) => {
                    let _ = writeln!(out, "W\tzoom\t{}", field(vectorcraft_ui_egui::i18n::t("Zoom")));
                }
                Node::Standard(_) => {}
                Node::Item(it) => {
                    let (ch, mods, text) = it.shortcut.map(shortcut).unwrap_or_default();
                    let _ = writeln!(
                        out,
                        "I\t{}\t{}\t{ch}\t{mods}\t{}\t{}\t{}",
                        entries.len(),
                        field(&it.label),
                        field(&text),
                        i32::from(it.enabled && it.command.is_some()),
                        checked(it.checked)
                    );
                    entries.push(it.clone());
                }
            }
        }
    }
    let mut out = String::new();
    let mut entries = Vec::new();
    for Menu { title, children, .. } in &bar.menus {
        let _ = writeln!(out, "M\t{}", field(title));
        walk(&mut out, &mut entries, children);
        out.push_str("E\n");
    }
    (out, entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn item(command: &'static str, label: &str, shortcut: Option<&'static str>) -> Node {
        Node::Item(Item { command: Some(command), params: Value::Null, source: "", label: label.into(), shortcut, enabled: true, checked: None })
    }

    fn mac_bar() -> MenuBar {
        let menu = |title: &str, role, children| Menu { title: title.into(), role, children };
        MenuBar {
            menus: vec![
                menu(
                    "VectorCraft",
                    MenuRole::App,
                    vec![
                        item(ABOUT, "About VectorCraft", None),
                        Node::Separator,
                        Node::Submenu { label: "Settings".into(), children: vec![item("edit.preferences", "General…", Some("Cmd+K"))] },
                        Node::Submenu { label: "Language".into(), children: vec![item("app.language", "Automatic", None)] },
                        Node::Separator,
                        Node::Standard(Standard::Services),
                        Node::Separator,
                        item(native_menu::HIDE, "Hide VectorCraft", Some("Ctrl+Cmd+H")),
                        Node::Standard(Standard::ShowAll),
                        Node::Separator,
                        item(QUIT, "Quit VectorCraft", Some("Cmd+Q")),
                    ],
                ),
                menu("File", MenuRole::Normal, vec![item("file.new", "New…", Some("Cmd+N"))]),
                menu("Edit", MenuRole::Normal, vec![item("edit.undo", "Undo", Some("Cmd+Z"))]),
                menu("Window", MenuRole::Window, vec![item(native_menu::MINIMIZE, "Minimize", None), Node::Standard(Standard::Zoom), Node::Separator, Node::Standard(Standard::BringAllToFront)]),
                menu("Help", MenuRole::Help, vec![item("help.checkForUpdates", "Check for Updates…", None), item("help.docs", "Help", Some("F1"))]),
            ],
        }
    }

    fn ids(bar: &MenuBar, title: &str) -> Vec<String> {
        let m = bar.menus.iter().find(|m| m.title == title).unwrap();
        m.children
            .iter()
            .map(|n| match n {
                Node::Item(it) => it.command.unwrap_or("?").to_string(),
                Node::Submenu { label, .. } => format!("[{label}]"),
                Node::Separator => "-".into(),
                Node::Header(h) => format!("#{h}"),
                Node::Standard(s) => format!("{s:?}"),
            })
            .collect()
    }

    #[test]
    fn the_app_menu_goes_back_where_haiku_apps_keep_its_items() {
        let bar = haiku_layout(&mac_bar());
        let titles: Vec<&str> = bar.menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["File", "Edit", "Window", "Help"]);
        assert_eq!(ids(&bar, "File"), ["file.new", "-", QUIT]);
        assert_eq!(ids(&bar, "Edit"), ["edit.undo", "-", "[Settings]"]);
        assert_eq!(ids(&bar, "Window"), [native_menu::MINIMIZE, "Zoom", "-", "[Language]"]);
        assert_eq!(ids(&bar, "Help"), ["help.docs", "-", ABOUT]);
    }

    #[test]
    fn command_shortcuts_are_native_and_the_others_text() {
        assert_eq!(shortcut("Cmd+Shift+Z"), (b'Z', COMMAND | SHIFT, String::new()));
        assert_eq!(shortcut("Cmd+Alt+Ctrl+["), (b'[', COMMAND | CONTROL | OPTION, String::new()));
        assert_eq!(shortcut("Cmd++"), (b'+', COMMAND, String::new()));
        assert_eq!(shortcut("Cmd+Delete"), (0x7f, COMMAND, String::new()));
        assert_eq!(shortcut("Shift+F5"), (0, 0, "Shift+F5".into()));
        // A tool's bare key must not become a Command shortcut (B would be Command+B).
        assert_eq!(shortcut("B"), (0, 0, "B".into()));
    }

    #[derive(Default)]
    struct Recorder(Rc<RefCell<Vec<(bool, String)>>>);
    impl Sink for Recorder {
        fn build(&mut self, spec: &str) {
            self.0.borrow_mut().push((true, spec.into()));
        }
        fn update(&mut self, spec: &str) {
            self.0.borrow_mut().push((false, spec.into()));
        }
    }

    #[test]
    fn builds_once_then_updates_only_what_changed_and_reports_clicks() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let clicks: Clicks = Rc::default();
        let mut menu = HaikuMenu::new(Recorder(log.clone()), clicks.clone());
        let mut bar = mac_bar();
        menu.sync(&bar);
        menu.sync(&bar);
        {
            let log = log.borrow();
            assert_eq!(log.len(), 1, "an unchanged bar isn't sent again");
            let spec = &log[0].1;
            assert!(spec.starts_with("M\tFile\nI\t0\tNew…\t78\t2\t\t1\t-1\n-\nI\t1\tQuit VectorCraft\t81\t2\t\t1\t-1\n"), "{spec}");
            assert!(spec.contains("S\tSettings\nI\t3\tGeneral…\t75\t2\t\t1\t-1\nE\n"), "{spec}");
            assert!(spec.contains("\tHelp\t0\t0\tF1\t1\t-1\n"), "{spec}");
        }
        if let Node::Item(it) = &mut bar.menus[2].children[0] {
            it.enabled = false;
            it.label = "Undo Move".into();
        }
        menu.sync(&bar);
        assert_eq!(log.borrow().last().cloned(), Some((false, "2\tUndo Move\t0\t-1\n".into())));
        clicks.borrow_mut().extend([1, 99]);
        let events = menu.drain();
        assert!(matches!(events.as_slice(), [Event::Click(it)] if it.command == Some(QUIT)));
    }
}
