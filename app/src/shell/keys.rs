//! Every command Loupe has, its shortcut (Shortcuts.swift) and the menu bar (App.swift `.commands`).
//! Views implement the actions with `.on_action(cx.listener(..))`; the shell routes the ones it owns.
use std::collections::HashMap;

use gpui_kit::{
    Action, App, Global, KeyBinding, Keystroke, Menu, MenuItem, OsAction,
    component::input as text,
};

gpui_kit::actions!(loupe, [
    OpenRepository, GoToWorktree, ToggleSidebar, ChangesTab, HistoryTab, PullsTab, Refresh, Commit,
    NextFile, PreviousFile, MarkViewed, BackToList, FullMode, NavigateBack, NavigateForward,
    NextConflict, PreviousConflict, UseOurs, UseTheirs,
    // Not rebindable.
    Undo, Redo, OpenSettings, CheckForUpdates, Quit, Hide, HideOthers, ShowAll, CloseWindow, Minimize, Zoom,
    ToggleFullScreen, Fetch, Pull, Push, ShowBranches, NewWorktree,
    SelectWorktree1, SelectWorktree2, SelectWorktree3, SelectWorktree4, SelectWorktree5,
    SelectWorktree6, SelectWorktree7, SelectWorktree8, SelectWorktree9,
]);

// NSEvent.ModifierFlags raw values, so the "shortcuts" default the Swift app wrote reads back unchanged.
pub const SHIFT: u64 = 1 << 17;
pub const CONTROL: u64 = 1 << 18;
pub const OPTION: u64 = 1 << 19;
pub const COMMAND: u64 = 1 << 20;

/// A key plus modifiers, as stored in the "shortcuts" default and shown in menus.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: String,    // a lowercase character, or one of SPECIAL's names
    pub modifiers: u64, // COMMAND | SHIFT | OPTION | CONTROL
}

/// Swift name, gpui key name, glyph.
const SPECIAL: [(&str, &str, &str); 8] = [
    ("up", "up", "↑"), ("down", "down", "↓"), ("left", "left", "←"), ("right", "right", "→"),
    ("return", "enter", "↩"), ("tab", "tab", "⇥"), ("space", "space", "Space"), ("delete", "backspace", "⌫"),
];

impl Shortcut {
    pub fn new(key: &str, modifiers: u64) -> Self { Self { key: key.into(), modifiers } }

    pub fn display(&self) -> String {
        let m = self.modifiers;
        let mut s = String::new();
        for (bit, g) in [(CONTROL, "⌃"), (OPTION, "⌥"), (SHIFT, "⇧"), (COMMAND, "⌘")] {
            if m & bit != 0 { s += g }
        }
        s + &SPECIAL.iter().find(|x| x.0 == self.key).map_or_else(|| self.key.to_uppercase(), |x| x.2.into())
    }

    /// gpui's keystroke syntax: "cmd-shift-s", "alt-down", "ctrl--".
    pub fn keystroke(&self) -> String {
        let m = self.modifiers;
        let mut s = String::new();
        for (bit, n) in [(CONTROL, "ctrl-"), (OPTION, "alt-"), (SHIFT, "shift-"), (COMMAND, "cmd-")] {
            if m & bit != 0 { s += n }
        }
        s + SPECIAL.iter().find(|x| x.0 == self.key).map_or(self.key.as_str(), |x| x.1)
    }

    /// The shortcut a key press types, or None for keys that can't be one (bare letters, modifiers alone).
    pub fn from_keystroke(k: &Keystroke) -> Option<Self> {
        let m = &k.modifiers;
        let mods = [(m.platform, COMMAND), (m.shift, SHIFT), (m.alt, OPTION), (m.control, CONTROL)]
            .iter().filter(|x| x.0).fold(0, |a, x| a | x.1);
        if let Some(x) = SPECIAL.iter().find(|x| x.1 == k.key) {
            return (mods != 0).then(|| Self::new(x.0, mods));
        }
        // gpui's `key` is the key without shift applied, so ⇧1 records as 1, not !.
        let c = k.key.to_lowercase();
        (c.chars().count() == 1 && !c.chars().any(char::is_control) && mods & (COMMAND | CONTROL | OPTION) != 0)
            .then(|| Self::new(&c, mods))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    OpenRepository, GoToWorktree, ToggleSidebar, ChangesTab, HistoryTab, PullsTab, Refresh, Commit, NextFile,
    PreviousFile, MarkViewed, Back, FullMode, NavigateBack, NavigateForward, NextConflict, PreviousConflict,
    UseOurs, UseTheirs,
}

impl ShortcutAction {
    pub const ALL: [Self; 19] = {
        use ShortcutAction::*;
        [OpenRepository, GoToWorktree, ToggleSidebar, ChangesTab, HistoryTab, PullsTab, Refresh, Commit, NextFile,
         PreviousFile, MarkViewed, Back, FullMode, NavigateBack, NavigateForward, NextConflict, PreviousConflict,
         UseOurs, UseTheirs]
    };
    pub const GROUPS: [&str; 4] = ["General", "Tabs", "Changes", "Review"];

    /// The key the "shortcuts" default uses (the Swift enum's raw value).
    pub fn id(self) -> &'static str {
        use ShortcutAction::*;
        match self {
            OpenRepository => "openRepository", GoToWorktree => "goToWorktree", ToggleSidebar => "toggleSidebar",
            ChangesTab => "changesTab", HistoryTab => "historyTab", PullsTab => "pullsTab", Refresh => "refresh",
            Commit => "commit", NextFile => "nextFile", PreviousFile => "previousFile", MarkViewed => "markViewed",
            Back => "back", FullMode => "fullMode", NavigateBack => "navigateBack", NavigateForward => "navigateForward",
            NextConflict => "nextConflict", PreviousConflict => "previousConflict", UseOurs => "useOurs",
            UseTheirs => "useTheirs",
        }
    }

    pub fn title(self) -> &'static str {
        use ShortcutAction::*;
        match self {
            OpenRepository => "Open Repository", GoToWorktree => "Go to Worktree", ToggleSidebar => "Toggle Sidebar",
            ChangesTab => "Changes", HistoryTab => "History", PullsTab => "Pull Requests", Refresh => "Refresh",
            Commit => "Commit", NextFile => "Next File", PreviousFile => "Previous File",
            MarkViewed => "Mark Viewed & Next", Back => "Back to List", FullMode => "Compact / Full Mode",
            NavigateBack => "Navigate Back", NavigateForward => "Navigate Forward", NextConflict => "Next Conflict",
            PreviousConflict => "Previous Conflict", UseOurs => "Use Ours for Conflict",
            UseTheirs => "Use Theirs for Conflict",
        }
    }

    pub fn group(self) -> &'static str {
        use ShortcutAction::*;
        match self {
            OpenRepository | GoToWorktree | ToggleSidebar | Refresh => "General",
            ChangesTab | HistoryTab | PullsTab => "Tabs",
            Commit | NextConflict | PreviousConflict | UseOurs | UseTheirs => "Changes",
            NextFile | PreviousFile | MarkViewed | Back | FullMode | NavigateBack | NavigateForward => "Review",
        }
    }

    pub fn standard(self) -> Option<Shortcut> {
        use ShortcutAction::*;
        let s = Shortcut::new;
        Some(match self {
            OpenRepository => s("o", COMMAND),
            GoToWorktree => s("k", COMMAND),
            ToggleSidebar => s("s", COMMAND | SHIFT),
            ChangesTab => s("1", COMMAND),
            HistoryTab => s("2", COMMAND),
            PullsTab => s("3", COMMAND),
            Refresh => s("r", COMMAND),
            Commit => s("return", COMMAND),
            NextFile => s("down", OPTION),
            PreviousFile => s("up", OPTION),
            MarkViewed => s("v", COMMAND | SHIFT),
            Back => s("[", COMMAND),
            FullMode => s("f", COMMAND | OPTION),
            NavigateBack => s("-", CONTROL),
            NavigateForward => s("-", CONTROL | SHIFT),
            NextConflict => s("down", COMMAND | OPTION),
            PreviousConflict => s("up", COMMAND | OPTION),
            UseOurs => s("left", COMMAND | OPTION),
            UseTheirs => s("right", COMMAND | OPTION),
        })
    }

    pub fn action(self) -> Box<dyn Action> {
        use ShortcutAction as S;
        match self {
            S::OpenRepository => Box::new(OpenRepository), S::GoToWorktree => Box::new(GoToWorktree),
            S::ToggleSidebar => Box::new(ToggleSidebar), S::ChangesTab => Box::new(ChangesTab),
            S::HistoryTab => Box::new(HistoryTab), S::PullsTab => Box::new(PullsTab), S::Refresh => Box::new(Refresh),
            S::Commit => Box::new(Commit), S::NextFile => Box::new(NextFile), S::PreviousFile => Box::new(PreviousFile),
            S::MarkViewed => Box::new(MarkViewed), S::Back => Box::new(BackToList), S::FullMode => Box::new(FullMode),
            S::NavigateBack => Box::new(NavigateBack), S::NavigateForward => Box::new(NavigateForward),
            S::NextConflict => Box::new(NextConflict), S::PreviousConflict => Box::new(PreviousConflict),
            S::UseOurs => Box::new(UseOurs), S::UseTheirs => Box::new(UseTheirs),
        }
    }
}

/// The user's shortcuts: only what differs from the defaults; an entry with None means "no shortcut". Saved under
/// the "shortcuts" setting by `install`, as the Swift app's JSON ({id: {key, modifiers} | null}).
#[derive(Default)]
pub struct Shortcuts {
    pub custom: HashMap<String, Option<Shortcut>>,
}
impl Global for Shortcuts {}

impl Shortcuts {
    pub fn shortcut(&self, a: ShortcutAction) -> Option<Shortcut> {
        match self.custom.get(a.id()) { Some(e) => e.clone(), None => a.standard() }
    }
    pub fn is_custom(&self, a: ShortcutAction) -> bool { self.custom.contains_key(a.id()) }

    /// Sets `a`'s shortcut; any other action that had the same one loses it. Returns that action.
    pub fn set(&mut self, s: Option<Shortcut>, a: ShortcutAction) -> Option<ShortcutAction> {
        let taken = s.as_ref().and_then(|s| ShortcutAction::ALL.into_iter().find(|&b| b != a && self.shortcut(b).as_ref() == Some(s)));
        if let Some(t) = taken { self.custom.insert(t.id().into(), None); }
        if s == a.standard() { self.custom.remove(a.id()); } else { self.custom.insert(a.id().into(), s); }
        taken
    }

    pub fn reset(&mut self) { self.custom.clear() }
}

/// (Re)binds every Loupe key and rebuilds the menu bar, so a changed shortcut applies at once. Bindings other
/// crates registered (text input editing keys) are kept.
pub fn install(cx: &mut App) {
    if !cx.has_global::<Shortcuts>() { cx.set_global(Shortcuts::default()) }
    save_shortcuts(cx.global::<Shortcuts>());
    let keymap = cx.key_bindings();
    let others: Vec<KeyBinding> = keymap.borrow().bindings().filter(|b| !b.action().name().starts_with("loupe::")).cloned().collect();
    cx.clear_key_bindings();
    cx.bind_keys(others);

    let mut ours: Vec<(String, Box<dyn Action>)> = ShortcutAction::ALL.iter()
        .filter_map(|&a| cx.global::<Shortcuts>().shortcut(a).map(|s| (s.keystroke(), a.action())))
        .collect();
    let fixed: [(&str, Box<dyn Action>); 7] = [
        ("cmd-,", Box::new(OpenSettings)),
        ("cmd-q", Box::new(Quit)), ("cmd-h", Box::new(Hide)), ("alt-cmd-h", Box::new(HideOthers)),
        ("cmd-w", Box::new(CloseWindow)), ("cmd-m", Box::new(Minimize)), ("ctrl-cmd-f", Box::new(ToggleFullScreen)),
    ];
    ours.extend(fixed.into_iter().map(|(k, a)| (k.to_string(), a)));
    // The sidebar's ⌥⌘1…9 hints: the n-th worktree shown.
    ours.extend((0..9).filter_map(|i| select_worktree_action(i).map(|a| (format!("alt-cmd-{}", i + 1), a))));
    let mut bindings: Vec<KeyBinding> = ours.into_iter().filter_map(|(k, a)| KeyBinding::load(&k, a, None, false, None, cx).ok()).collect();
    // CH-31 / KB-14: a context-free binding ties the text input's own ⌘Z at the deepest depth and, bound later,
    // wins it. "!Input" leaves a focused field (and the merge result editor) its own Undo/Redo; anywhere else ⌘Z
    // is the worktree's git undo.
    bindings.push(KeyBinding::new("cmd-z", Undo, Some(NOT_IN_TEXT)));
    bindings.push(KeyBinding::new("cmd-shift-z", Redo, Some(NOT_IN_TEXT)));
    cx.bind_keys(bindings);
    let title = cx.try_global::<UndoTitle>().map_or_else(|| "Undo".to_string(), |t| t.0.clone());
    cx.set_menus(menus(&title));
}

/// Where ⌘Z / ⇧⌘Z are Loupe's: outside gpui-component's text inputs (their key context is "Input").
pub const NOT_IN_TEXT: &str = "!Input";

/// Edit › Undo reads "Undo <label>" for the selected worktree's newest action (UndoCommands in Undo.swift).
struct UndoTitle(String);
impl Global for UndoTitle {}

/// Rebuilds the menu bar when the title changed; cheap to call on every model change.
pub fn set_undo_title(title: String, cx: &mut App) {
    if cx.try_global::<UndoTitle>().is_some_and(|t| t.0 == title) { return }
    cx.set_menus(menus(&title));
    cx.set_global(UndoTitle(title));
}

/// Reads the "shortcuts" setting into the `Shortcuts` global (before the first `install`).
pub fn load_shortcuts(cx: &mut App) {
    let data = loupe_forge::settings::shared().data("shortcuts").map(<[u8]>::to_vec);
    cx.set_global(Shortcuts { custom: data.map(|d| parse_shortcuts(&d)).unwrap_or_default() });
}

fn parse_shortcuts(d: &[u8]) -> HashMap<String, Option<Shortcut>> {
    let Ok(serde_json::Value::Object(m)) = serde_json::from_slice(d) else { return HashMap::new() };
    m.into_iter().map(|(id, v)| {
        let s = v.get("key").and_then(|k| k.as_str()).map(|k| Shortcut::new(k, v.get("modifiers").and_then(|m| m.as_u64()).unwrap_or(0)));
        (id, s)
    }).collect()
}

fn save_shortcuts(s: &Shortcuts) {
    let m: serde_json::Map<String, serde_json::Value> = s.custom.iter().map(|(id, s)| (id.clone(), match s {
        Some(s) => serde_json::json!({ "key": s.key, "modifiers": s.modifiers }),
        None => serde_json::Value::Null,
    })).collect();
    let mut p = loupe_forge::settings::shared();
    // Unchanged JSON still compares equal (object keys are sorted), so this writes only after a change.
    p.set("shortcuts", Some(plist::Value::Data(serde_json::to_vec(&m).unwrap_or_default())));
}

pub fn select_worktree_action(n: usize) -> Option<Box<dyn Action>> {
    let all: [Box<dyn Action>; 9] = [Box::new(SelectWorktree1), Box::new(SelectWorktree2), Box::new(SelectWorktree3),
        Box::new(SelectWorktree4), Box::new(SelectWorktree5), Box::new(SelectWorktree6), Box::new(SelectWorktree7),
        Box::new(SelectWorktree8), Box::new(SelectWorktree9)];
    all.into_iter().nth(n)
}

fn menus(undo: &str) -> Vec<Menu> {
    fn a(name: &'static str, action: impl Action) -> MenuItem { MenuItem::action(name, action) }
    vec![
        // No "Check for Updates…": the Swift app hides it when Sparkle isn't configured, and this build has no updater.
        Menu::new("Loupe").items([
            a("Settings…", OpenSettings), MenuItem::separator(),
            a("Hide Loupe", Hide), a("Hide Others", HideOthers), a("Show All", ShowAll), MenuItem::separator(),
            a("Quit Loupe", Quit),
        ]),
        Menu::new("File").items([a("Open Repository…", OpenRepository), MenuItem::separator(), a("Close Window", CloseWindow)]),
        Menu::new("Edit").items([
            MenuItem::os_action(undo.to_string(), Undo, OsAction::Undo), MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", text::Cut, OsAction::Cut), MenuItem::os_action("Copy", text::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", text::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", text::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            a("Toggle Sidebar", ToggleSidebar), MenuItem::separator(),
            a("Changes", ChangesTab), a("History", HistoryTab), a("Pull Requests", PullsTab), MenuItem::separator(),
            // No "Enter Full Screen" here: AppKit adds its own to a menu named View (as in Swift), so ours was a
            // second copy. ⌃⌘F stays bound to ToggleFullScreen.
            a("Go to Worktree…", GoToWorktree),
        ]),
        Menu::new("Review").items([
            a("Next File", NextFile), a("Previous File", PreviousFile), a("Mark Viewed & Next", MarkViewed),
            a("Compact / Full Mode", FullMode), MenuItem::separator(), a("Refresh", Refresh),
        ]),
        Menu::new("Window").items([a("Minimize", Minimize), a("Zoom", Zoom)]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_keystroke() {
        let s = ShortcutAction::ToggleSidebar.standard().unwrap();
        assert_eq!(s.display(), "⇧⌘S");
        assert_eq!(s.keystroke(), "shift-cmd-s");
        assert_eq!(ShortcutAction::NextConflict.standard().unwrap().display(), "⌥⌘↓");
        assert_eq!(ShortcutAction::Commit.standard().unwrap().keystroke(), "cmd-enter");
        assert_eq!(ShortcutAction::NavigateForward.standard().unwrap().display(), "⌃⇧-");
        for a in ShortcutAction::ALL {
            let k = a.standard().unwrap().keystroke();
            let parsed = Keystroke::parse(Box::leak(k.clone().into_boxed_str())).unwrap_or_else(|e| panic!("{k}: {e:?}"));
            assert_eq!(Shortcut::from_keystroke(&parsed), a.standard(), "{k}");
        }
    }

    #[test]
    fn undo_yields_to_text_inputs() {
        use gpui_kit::{KeyBindingContextPredicate, KeyContext};
        let p = KeyBindingContextPredicate::parse(NOT_IN_TEXT).unwrap();
        let ctx = |names: &[&str]| names.iter().map(|n| KeyContext::parse(n).unwrap()).collect::<Vec<_>>();
        assert!(p.depth_of(&ctx(&["Loupe", "ChangesList"])).is_some());
        assert!(p.depth_of(&ctx(&["Loupe", "MergeEditor", "Input"])).is_none());
    }

    #[test]
    fn reads_the_swift_shortcuts_json() {
        let m = parse_shortcuts(br#"{"refresh":{"key":"k","modifiers":1048576},"goToWorktree":null}"#);
        assert_eq!(m["refresh"], Some(Shortcut::new("k", COMMAND)));
        assert_eq!(m["goToWorktree"], None);
        assert!(parse_shortcuts(b"junk").is_empty());
    }

    #[test]
    fn recording_rejects_bare_keys() {
        assert_eq!(Shortcut::from_keystroke(&Keystroke::parse("a").unwrap()), None);
        assert_eq!(Shortcut::from_keystroke(&Keystroke::parse("shift-a").unwrap()), None);
        assert_eq!(Shortcut::from_keystroke(&Keystroke::parse("down").unwrap()), None);
        assert_eq!(Shortcut::from_keystroke(&Keystroke::parse("shift-down").unwrap()), Some(Shortcut::new("down", SHIFT)));
        assert_eq!(Shortcut::from_keystroke(&Keystroke::parse("cmd-shift-1").unwrap()), Some(Shortcut::new("1", COMMAND | SHIFT)));
    }

    #[test]
    fn setting_a_taken_shortcut_moves_it() {
        let mut s = Shortcuts::default();
        let k = Shortcut::new("k", COMMAND);
        assert_eq!(s.set(Some(k.clone()), ShortcutAction::Refresh), Some(ShortcutAction::GoToWorktree));
        assert_eq!(s.shortcut(ShortcutAction::Refresh), Some(k));
        assert_eq!(s.shortcut(ShortcutAction::GoToWorktree), None);
        assert!(s.is_custom(ShortcutAction::GoToWorktree));
        // Back to the default: no longer custom.
        s.set(ShortcutAction::Refresh.standard(), ShortcutAction::Refresh);
        assert!(!s.is_custom(ShortcutAction::Refresh));
        s.reset();
        assert_eq!(s.shortcut(ShortcutAction::GoToWorktree), ShortcutAction::GoToWorktree.standard());
    }
}
