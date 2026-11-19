//! Pages, drawn the way the macOS app lays them out: a glass sidebar on a gradient that
//! belongs to the page, content on glass panels, the big round button centred at the
//! bottom with pill buttons beside it. Each paint registers the areas that act on a click.

use super::gfx::{Align, Canvas, Fonts, R, WHITE, amber, argb, backdrop, dim, fg, hex, light, mint};
use super::jobs::{self, Job};
use super::{confirm, invalidate, reveal, style_frame, timers};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use windows_sys::Win32::Foundation::HWND;

// ---------------------------------------------------------------- modules

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    Dashboard,
    Junk,
    Installers,
    Projects,
    Uninstaller,
    SpaceLens,
    LargeOld,
    Duplicates,
    History,
    Settings,
}

/// The pages built on a pick list (find, tick, act).
const PICK_MODULES: [Module; 4] = [Module::Installers, Module::Projects, Module::LargeOld, Module::Duplicates];

/// Sidebar: dashboard, then groups, then history (as on macOS).
const SECTIONS: &[(&str, &[Module])] = &[
    ("", &[Module::Dashboard]),
    ("Cleanup", &[Module::Junk, Module::Installers, Module::Projects]),
    ("Applications", &[Module::Uninstaller]),
    ("Files", &[Module::SpaceLens, Module::LargeOld, Module::Duplicates]),
    (" ", &[Module::History, Module::Settings]),
];

struct ToolText {
    title: String,
    subtitle: String,
    points: Vec<String>,
    colors: (u32, u32),
}

/// Titles, texts and colors of the cleaning tools, shared with macOS (`shared/tools.json`).
fn tool(id: &str) -> Option<ToolText> {
    let all: Value = serde_json::from_str(include_str!("../../../shared/tools.json")).ok()?;
    let t = all["tools"].as_array()?.iter().find(|t| t["id"] == id)?;
    let colors: Vec<u32> = t["colors"].as_array()?.iter().filter_map(|c| c.as_str()).map(hex).collect();
    Some(ToolText {
        title: t["title"].as_str()?.into(),
        subtitle: t["subtitle"].as_str().unwrap_or("").into(),
        points: t["points"].as_array().map(|p| p.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        colors: (*colors.first().unwrap_or(&0x4D5B73), *colors.get(1).unwrap_or(&0x1C2233)),
    })
}

impl Module {
    fn tool_id(self) -> Option<&'static str> {
        match self {
            Module::Junk => Some("clean"),
            Module::Installers => Some("installer"),
            Module::Projects => Some("purge"),
            _ => None,
        }
    }

    pub fn title(self) -> String {
        if let Some(t) = self.tool_id().and_then(tool) {
            return t.title;
        }
        match self {
            Module::Dashboard => "Dashboard",
            Module::Uninstaller => "Uninstaller",
            Module::SpaceLens => "Space Lens",
            Module::LargeOld => "Large & Old Files",
            Module::Duplicates => "Duplicates",
            Module::History => "History",
            Module::Settings => "Settings",
            _ => "",
        }
        .into()
    }

    pub fn palette(self) -> (u32, u32) {
        if let Some(t) = self.tool_id().and_then(tool) {
            return t.colors;
        }
        match self {
            Module::Dashboard => (0x3A3FB8, 0x1B1446),
            Module::SpaceLens => (0x0E8A9E, 0x123063),
            Module::Uninstaller => (0x2F6FD6, 0x2A1857),
            Module::LargeOld => (0x9A5B13, 0x3B1D52),
            Module::Duplicates => (0x1F7A8C, 0x2B1B5C),
            _ => (0x4D5B73, 0x1C2233),
        }
    }

    /// Segoe Fluent Icons / MDL2 glyphs.
    fn glyph(self) -> char {
        match self {
            Module::Dashboard => '\u{E80F}',
            Module::Junk => '\u{E74D}',
            Module::Installers => '\u{E896}',
            Module::Projects => '\u{E943}',
            Module::Uninstaller => '\u{E8BB}',
            Module::SpaceLens => '\u{EB05}',
            Module::LargeOld => '\u{E823}',
            Module::Duplicates => '\u{E8C8}',
            Module::History => '\u{E81C}',
            Module::Settings => '\u{E713}',
        }
    }

    /// Title, text and points of the page's intro. The file pages are Windows-only so far,
    /// hence not in `shared/tools.json`.
    fn intro(self) -> (String, String, Vec<String>) {
        if let Some(t) = self.tool_id().and_then(tool) {
            return (t.title, t.subtitle, t.points);
        }
        let (subtitle, points): (&str, &[&str]) = match self {
            Module::LargeOld => (
                "Find big files nobody opened for months, and old screenshots, in your own folders.",
                &["Files of 500 MB or more, untouched for 180 days", "Everything goes to the Recycle Bin"],
            ),
            Module::Duplicates => (
                "Find identical copies of the same file in Downloads, Documents, Desktop and your media folders.",
                &["Compares contents, not just names", "The extra copies are picked, the first one stays"],
            ),
            _ => ("", &[]),
        };
        (self.title(), subtitle.into(), points.iter().map(|p| p.to_string()).collect())
    }

    /// The engine request that finds a pick list's items.
    fn find_request(self) -> Value {
        match self {
            Module::Installers => json!({"op": "installers"}),
            Module::LargeOld => json!({"op": "large_old"}),
            Module::Duplicates => json!({"op": "duplicates"}),
            _ => json!({"op": "projects"}),
        }
    }

    /// Picked items go to the Recycle Bin (everything but rebuildable build folders).
    fn trashes(self) -> bool {
        self != Module::Projects
    }
}

const CHECK: char = '\u{E73E}';
const BACK: char = '\u{E72B}';
const REFRESH: char = '\u{E72C}';
const CHEVRON: char = '\u{E76C}';
const FOLDER: char = '\u{E8B7}';
const FILE: char = '\u{E7C3}';
const DRIVE: char = '\u{EDA2}';
const OPENED: char = '\u{E70D}';
const MINUS: char = '\u{E738}';

// ---------------------------------------------------------------- actions

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Go(Module),
    DashRefresh,
    SpaceOpen(Option<String>),
    SpaceBack,
    SpaceStartOver,
    SpaceRefresh,
    SpaceToggle(String),
    SpaceRemove,
    Reveal(String),
    JunkScan,
    JunkStop,
    JunkToggle(String),
    /// show or hide a rule's items (rule id)
    JunkExpand(String),
    /// rule id, item path
    JunkItem(String, String),
    /// item path: show or hide its children
    JunkOpen(String),
    /// rule id, item path, child path
    JunkPart(String, String, String),
    JunkClean,
    JunkReset,
    PickFind(Module),
    PickStop(Module),
    PickToggle(Module, usize),
    PickOpen(Module, usize),
    PickPart(Module, usize, String),
    PickAct(Module),
    PickReset(Module),
    AppToggle(String),
    AppsUninstall,
    AppsReload,
    FastToggle,
}

impl Action {
    /// Ticks something in a list: these take keyboard focus (Up/Down, Space).
    fn toggles(&self) -> bool {
        matches!(
            self,
            Action::JunkToggle(_)
                | Action::JunkItem(..)
                | Action::JunkPart(..)
                | Action::PickToggle(..)
                | Action::PickPart(..)
                | Action::AppToggle(_)
                | Action::SpaceToggle(_)
                | Action::FastToggle
        )
    }
}

// ---------------------------------------------------------------- page state

#[derive(Default)]
struct Dash {
    status: Value,
    job: Option<Job>,
}

#[derive(Default)]
struct Space {
    /// folders opened; None is the startup-drive overview
    stack: Vec<Option<String>>,
    listings: HashMap<String, Value>,
    job: Option<(String, Job)>,
    selected: HashSet<String>,
    remove_job: Option<Job>,
    error: Option<String>,
    scroll: f32,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Finding,
    Review,
    Acting,
    Done,
}

/// Listed folders the user opened, their children (from the engine), and the children kept
/// ticked. A folder missing from `ticked` goes whole; present, only those children go.
#[derive(Default)]
struct Parts {
    open: HashSet<String>,
    children: HashMap<String, Vec<Value>>,
    ticked: HashMap<String, HashSet<String>>,
    /// children being listed, by folder
    loading: Vec<(String, Job)>,
    /// whether a listed path is a folder (asked once: rows are painted often)
    dirs: HashMap<String, bool>,
}

impl Parts {
    /// A plain folder can be opened; files and links cannot.
    fn can_open(&mut self, path: &str) -> bool {
        *self.dirs.entry(path.to_string()).or_insert_with(|| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()))
    }

    /// Bytes going from `parent`: its own size when whole, else its ticked children.
    fn bytes(&self, parent: &str, whole: f64) -> f64 {
        let Some(t) = self.ticked.get(parent) else { return whole };
        self.children
            .get(parent)
            .into_iter()
            .flatten()
            .filter(|c| c["path"].as_str().is_some_and(|p| t.contains(p)))
            .filter_map(|c| c["size"].as_f64())
            .sum()
    }

    fn is_partial(&self, parent: &str) -> bool {
        self.ticked.contains_key(parent)
    }

    fn is_ticked(&self, child: &str, parent: &str) -> bool {
        self.ticked.get(parent).is_none_or(|t| t.contains(child))
    }

    /// Ticks or unticks one child of a ticked folder. False when none is left: the caller
    /// unticks the folder itself.
    fn toggle(&mut self, child: &str, parent: &str) -> bool {
        let all: HashSet<String> = self.children.get(parent).into_iter().flatten().filter_map(|c| c["path"].as_str().map(String::from)).collect();
        let mut t = self.ticked.get(parent).cloned().unwrap_or_else(|| all.clone());
        if !t.remove(child) {
            t.insert(child.to_string());
        }
        let left = !t.is_empty();
        if t == all || t.is_empty() {
            self.ticked.remove(parent);
        } else {
            self.ticked.insert(parent.to_string(), t);
        }
        left
    }

    /// Ticking a child of an unticked folder: that child only.
    fn only(&mut self, child: &str, parent: &str) {
        if self.children.get(parent).map_or(0, Vec::len) > 1 {
            self.ticked.insert(parent.to_string(), HashSet::from([child.to_string()]));
        } else {
            self.ticked.remove(parent);
        }
    }

    fn whole(&mut self, parent: &str) {
        self.ticked.remove(parent);
    }

    /// Opens or closes `parent`; the first time, the engine lists its children.
    fn toggle_open(&mut self, parent: &str, hwnd: HWND) {
        if self.open.remove(parent) {
            return;
        }
        self.open.insert(parent.to_string());
        if !self.children.contains_key(parent) && !self.loading.iter().any(|(p, _)| p == parent) {
            self.loading.push((parent.to_string(), jobs::start(hwnd, json!({"op": "space", "path": parent}))));
        }
    }

    /// Takes the events of its listing jobs; false when `job` is not one of them.
    fn on_event(&mut self, job: u64, e: &Value) -> bool {
        let Some(i) = self.loading.iter().position(|(_, j)| j.id == job) else { return false };
        let parent = self.loading[i].0.clone();
        match e["event"].as_str() {
            // a folder that cannot be listed shows as empty
            Some("space") => {
                self.children.insert(parent, e["entries"].as_array().cloned().unwrap_or_default());
            }
            Some("done") => {
                self.loading.remove(i);
                self.children.entry(parent).or_default();
            }
            _ => {}
        }
        true
    }

    /// The `parts` of a clean request: folders in `chosen` with only some children ticked.
    fn request(&self, chosen: impl Fn(&str) -> bool) -> HashMap<String, Vec<String>> {
        self.ticked.iter().filter(|(k, _)| chosen(k)).map(|(k, t)| (k.clone(), t.iter().cloned().collect())).collect()
    }
}

impl Drop for Parts {
    fn drop(&mut self) {
        for (_, j) in &self.loading {
            j.cancel();
        }
    }
}

#[derive(Default)]
struct Junk {
    phase: Phase,
    plan: Value,
    selected: HashSet<String>,
    /// items unticked inside a selected rule (paths)
    excluded: HashSet<String>,
    /// rules whose items are shown
    expanded: HashSet<String>,
    parts: Parts,
    progress: Value,
    outcome: Value,
    job: Option<Job>,
    scroll: f32,
}

#[derive(Default)]
struct Picks {
    phase: Phase,
    list: Value,
    selected: HashSet<usize>,
    parts: Parts,
    progress: Value,
    outcome: Value,
    job: Option<Job>,
    scroll: f32,
}

#[derive(Default)]
struct Apps {
    list: Vec<Value>,
    sizes: Vec<u64>,
    loaded: bool,
    chosen: HashSet<String>,
    message: Option<String>,
    job: Option<Job>,
    scroll: f32,
}

#[derive(Default)]
struct History {
    sessions: Vec<Value>,
    log: Option<String>,
    scroll: f32,
}

pub struct State {
    hwnd: HWND,
    pub fonts: Fonts,
    module: Module,
    hits: Vec<(R, Action)>,
    mouse: (f32, f32),
    /// the scrollable area of this paint and how tall its content is
    view: Option<(R, f32)>,
    /// while a scroll panel draws, its visible area: clicks never reach beyond it
    hit_clip: Option<R>,
    version: String,
    started: Instant,
    dash: Dash,
    space: Space,
    junk: Junk,
    installers: Picks,
    projects: Picks,
    large: Picks,
    dups: Picks,
    apps: Apps,
    history: History,
    /// Settings: scans and removals take every core
    fast: bool,
    /// this paint's tick boxes, unclipped, in list order (keyboard focus moves along them)
    toggles: Vec<(R, Action)>,
    focus: Option<Action>,
}

const FAST_VALUE: &str = "FastScans";

/// Tells the engine how hard to work; returns `fast`.
fn set_fast(fast: bool) -> bool {
    jobs::call(json!({"op": "priority", "fast": fast}));
    fast
}

impl State {
    pub fn new(hwnd: HWND) -> State {
        State {
            hwnd,
            fonts: Fonts::default(),
            module: Module::Dashboard,
            hits: Vec::new(),
            mouse: (-1.0, -1.0),
            view: None,
            hit_clip: None,
            version: jobs::call(json!({"op": "version"}))["version"].as_str().unwrap_or("").into(),
            started: Instant::now(),
            dash: Dash::default(),
            space: Space::default(),
            junk: Junk::default(),
            installers: Picks::default(),
            projects: Picks::default(),
            large: Picks::default(),
            dups: Picks::default(),
            apps: Apps::default(),
            history: History::default(),
            fast: set_fast(super::reg_dword(super::SETTINGS_KEY, FAST_VALUE) == Some(1)),
            toggles: Vec::new(),
            focus: None,
        }
    }

    fn picks(&mut self, m: Module) -> &mut Picks {
        match m {
            Module::Installers => &mut self.installers,
            Module::LargeOld => &mut self.large,
            Module::Duplicates => &mut self.dups,
            _ => &mut self.projects,
        }
    }

    fn pick(&self, m: Module) -> &Picks {
        match m {
            Module::Installers => &self.installers,
            Module::LargeOld => &self.large,
            Module::Duplicates => &self.dups,
            _ => &self.projects,
        }
    }

    fn parts(&mut self, m: Module) -> &mut Parts {
        if m == Module::Junk { &mut self.junk.parts } else { &mut self.picks(m).parts }
    }

    /// The title bar's color for the current page.
    pub fn caption(&self) -> u32 {
        self.module.palette().0
    }

    fn busy(&self) -> bool {
        self.dash.job.is_some()
            || self.space.job.is_some()
            || self.space.remove_job.is_some()
            || self.junk.job.is_some()
            || PICK_MODULES.iter().any(|&m| self.pick(m).job.is_some())
            || self.apps.job.is_some()
    }

    fn update_timers(&self) {
        timers(self.hwnd, self.busy(), self.module == Module::Dashboard);
    }

    // ------------------------------------------------------------ input

    /// Returns true when the hovered action changed (repaint).
    pub fn hover_at(&mut self, x: f32, y: f32) -> bool {
        let before = self.hits.iter().position(|(r, _)| r.contains(self.mouse.0, self.mouse.1));
        self.mouse = (x, y);
        let after = self.hits.iter().position(|(r, _)| r.contains(x, y));
        before != after || after.is_some()
    }

    pub fn hovering_action(&self) -> bool {
        self.hits.iter().any(|(r, _)| r.contains(self.mouse.0, self.mouse.1))
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<Action> {
        // later registrations are on top
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, a)| a.clone())
    }

    pub fn scroll_by(&mut self, dy: f32) {
        let Some((view, content)) = self.view else { return };
        let max = (content - view.h).max(0.0);
        let s = match self.module {
            Module::SpaceLens => &mut self.space.scroll,
            Module::Junk => &mut self.junk.scroll,
            m @ (Module::Installers | Module::Projects | Module::LargeOld | Module::Duplicates) => &mut self.picks(m).scroll,
            Module::Uninstaller => &mut self.apps.scroll,
            Module::History => &mut self.history.scroll,
            Module::Dashboard | Module::Settings => return,
        };
        *s = (*s + dy).clamp(0.0, max);
    }

    fn hit(&mut self, r: R, a: Action) {
        if a.toggles() {
            self.toggles.push((r, a.clone()));
        }
        let r = match self.hit_clip {
            Some(c) => {
                let (x, y) = (r.x.max(c.x), r.y.max(c.y));
                let (right, bottom) = (r.right().min(c.right()), r.bottom().min(c.bottom()));
                if right <= x || bottom <= y {
                    return;
                }
                R::new(x, y, right - x, bottom - y)
            }
            None => r,
        };
        self.hits.push((r, a));
    }

    fn hot(&self, r: R) -> bool {
        r.contains(self.mouse.0, self.mouse.1)
    }

    pub fn on_show_module(&mut self) {
        match self.module {
            Module::Dashboard => self.refresh_dashboard(),
            Module::Uninstaller if !self.apps.loaded && self.apps.job.is_none() => self.load_apps(),
            Module::History => self.load_history(),
            _ => {}
        }
        style_frame(self.hwnd, self.module.palette().0);
        // the page in the title: what a screen reader announces for this painted window
        let title = super::wide(&format!("OpenCleaner - {}", self.module.title()));
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetWindowTextW(self.hwnd, title.as_ptr()) };
        self.focus = None;
        self.update_timers();
    }

    // ------------------------------------------------------------ keyboard

    /// Ctrl+R / F5 scan again, Esc stops a scan, Up/Down move along the tick boxes, Space
    /// ticks the one in focus. Returns what to perform.
    pub fn key(&mut self, vk: u16, ctrl: bool) -> Option<Action> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_F5, VK_SPACE, VK_UP};
        match vk {
            VK_F5 => self.rescan(),
            0x52 if ctrl => self.rescan(),
            VK_ESCAPE => match self.module {
                Module::Junk if self.junk.phase == Phase::Finding => Some(Action::JunkStop),
                m if PICK_MODULES.contains(&m) && self.pick(m).phase == Phase::Finding => Some(Action::PickStop(m)),
                _ => None,
            },
            VK_UP => self.move_focus(-1),
            VK_DOWN => self.move_focus(1),
            VK_SPACE => self.focus.clone().filter(|f| self.toggles.iter().any(|(_, a)| a == f)),
            _ => None,
        }
    }

    fn rescan(&self) -> Option<Action> {
        match self.module {
            Module::Dashboard => Some(Action::DashRefresh),
            Module::Junk if self.junk.job.is_none() => Some(Action::JunkScan),
            m if PICK_MODULES.contains(&m) && self.pick(m).job.is_none() => Some(Action::PickFind(m)),
            Module::Uninstaller if self.apps.job.is_none() => Some(Action::AppsReload),
            Module::SpaceLens if !self.space.stack.is_empty() => Some(Action::SpaceRefresh),
            _ => None,
        }
    }

    /// Moves the focus to the next or previous tick box and scrolls it into view.
    fn move_focus(&mut self, by: isize) -> Option<Action> {
        let last = self.toggles.len().checked_sub(1)?;
        let at = self.focus.as_ref().and_then(|f| self.toggles.iter().position(|(_, a)| a == f));
        let next = at.map_or(0, |i| i.saturating_add_signed(by).min(last));
        let (r, a) = self.toggles[next].clone();
        self.focus = Some(a);
        if let Some((view, _)) = self.view {
            if r.y < view.y {
                self.scroll_by(r.y - view.y - 4.0);
            } else if r.bottom() > view.bottom() {
                self.scroll_by(r.bottom() - view.bottom() + 4.0);
            }
        }
        None
    }

    pub fn refresh_dashboard(&mut self) {
        if self.dash.job.is_none() {
            self.dash.job = Some(jobs::start(self.hwnd, json!({"op": "status"})));
            self.update_timers();
        }
    }

    fn load_apps(&mut self) {
        self.apps.list.clear();
        self.apps.sizes.clear();
        self.apps.loaded = false;
        self.apps.job = Some(jobs::start(self.hwnd, json!({"op": "apps"})));
        self.update_timers();
    }

    fn load_history(&mut self) {
        let h = jobs::call(json!({"op": "history"}));
        self.history.sessions = h["sessions"].as_array().cloned().unwrap_or_default();
        self.history.log = h["logs"]["deletions"].as_str().map(String::from);
    }

    // ------------------------------------------------------------ engine events

    pub fn on_event(&mut self, job: u64, e: Value) {
        // children of a folder opened in a list
        if [Module::Junk].into_iter().chain(PICK_MODULES).any(|m| self.parts(m).on_event(job, &e)) {
            return;
        }
        let kind = e["event"].as_str().unwrap_or("").to_string();
        let is = |j: &Option<Job>| j.as_ref().is_some_and(|j| j.id == job);
        if is(&self.dash.job) {
            match kind.as_str() {
                "status" => self.dash.status = e,
                "done" => self.dash.job = None,
                _ => {}
            }
        } else if self.space.job.as_ref().is_some_and(|(_, j)| j.id == job) {
            let key = self.space.job.as_ref().map(|(k, _)| k.clone()).unwrap_or_default();
            match kind.as_str() {
                "progress" | "space" => {
                    if let Some(err) = e["error"].as_str() {
                        self.space.error = Some(err.into());
                    } else {
                        self.space.listings.insert(key, e);
                    }
                }
                "done" => self.space.job = None,
                _ => {}
            }
        } else if is(&self.space.remove_job) {
            if kind == "removed" {
                let failed = e["failed"].as_u64().unwrap_or(0);
                self.space.error = (failed > 0).then(|| format!("{failed} items could not be moved to the Recycle Bin"));
            }
            if kind == "done" {
                self.space.remove_job = None;
                self.space.listings.clear();
                self.space.selected.clear();
                let current = self.space.stack.last().cloned().flatten();
                self.space_load(current);
            }
        } else if is(&self.junk.job) {
            match kind.as_str() {
                "progress" if self.junk.phase == Phase::Finding => self.junk.plan = e,
                "progress" => self.junk.progress = e,
                "plan" => {
                    self.junk.selected = rules_of(&e)
                        .filter(|r| r["default"] == true && r["status"] == "ready")
                        .filter_map(|r| r["id"].as_str().map(String::from))
                        .collect();
                    self.junk.plan = e;
                    self.junk.phase = Phase::Review;
                    self.junk.scroll = 0.0;
                }
                "cleaned" => {
                    self.junk.outcome = e;
                    self.junk.phase = Phase::Done;
                }
                "done" => {
                    self.junk.job = None;
                    if self.junk.phase == Phase::Finding {
                        self.junk.phase = Phase::Idle;
                    }
                }
                _ => {}
            }
        } else if let Some(m) = PICK_MODULES.into_iter().find(|&m| is(&self.pick(m).job)) {
            let p = self.picks(m);
            match kind.as_str() {
                "picks" => {
                    p.selected = items_of(&e)
                        .enumerate()
                        .filter(|(_, it)| it["preselect"] == true)
                        .map(|(i, _)| i)
                        .collect();
                    p.list = e;
                    p.phase = Phase::Review;
                    p.scroll = 0.0;
                }
                "progress" => p.progress = e,
                "removed" => {
                    p.outcome = e;
                    p.phase = Phase::Done;
                }
                "done" => {
                    p.job = None;
                    if p.phase == Phase::Finding {
                        p.phase = Phase::Idle;
                    }
                }
                _ => {}
            }
        } else if is(&self.apps.job) {
            match kind.as_str() {
                "apps" => {
                    self.apps.list = e["apps"].as_array().cloned().unwrap_or_default();
                    self.apps.sizes = vec![0; self.apps.list.len()];
                    self.apps.loaded = true;
                }
                "sizes" => {
                    self.apps.sizes = e["sizes"].as_array().map(|a| a.iter().map(|v| v.as_u64().unwrap_or(0)).collect()).unwrap_or_default();
                }
                "done" => self.apps.job = None,
                _ => {}
            }
        }
        self.update_timers();
    }

    // ------------------------------------------------------------ painting

    pub fn draw(&mut self, c: &mut Canvas, w: f32, h: f32) {
        self.hits.clear();
        self.view = None;
        self.toggles.clear();
        let (from, to) = self.module.palette();
        c.gradient(R::new(0.0, 0.0, w, h), backdrop(from), backdrop(to));
        // two soft light blobs, so the glass has something to sit on
        c.circle(w * 0.85, h * 0.08, 360.0, Some(argb(from, if light() { 40 } else { 70 })), None);
        c.circle(w * 0.2, h * 0.98, 300.0, Some(argb(WHITE, 14)), None);

        self.draw_sidebar(c, R::new(14.0, 14.0, 236.0, h - 28.0));
        let page = R::new(264.0, 0.0, w - 264.0 - 14.0, h);
        match self.module {
            Module::Dashboard => self.draw_dashboard(c, page),
            Module::Junk => self.draw_junk(c, page),
            m @ (Module::Installers | Module::Projects | Module::LargeOld | Module::Duplicates) => self.draw_picks(c, page, m),
            Module::Uninstaller => self.draw_apps(c, page),
            Module::SpaceLens => self.draw_space(c, page),
            Module::History => self.draw_history(c, page),
            Module::Settings => self.draw_settings(c, page),
        }

        // keyboard focus ring, inside the list it belongs to
        if let Some(f) = &self.focus
            && let Some((r, _)) = self.toggles.iter().find(|(_, a)| a == f)
        {
            if let Some((v, _)) = self.view {
                c.clip(v);
            }
            c.rounded(*r, 12.0, None, Some(argb(fg(), 200)));
            c.unclip();
        }
    }

    fn draw_sidebar(&mut self, c: &mut Canvas, r: R) {
        c.glass(r, 22.0);
        let mut y = r.y + 10.0;
        for (group, modules) in SECTIONS {
            if !group.trim().is_empty() {
                c.text(R::new(r.x + 22.0, y + 8.0, r.w - 30.0, 20.0), group, 12.0, false, dim(), Align::Left);
                y += 30.0;
            } else if *group == " " {
                y += 12.0;
            }
            for m in modules.iter() {
                let row = R::new(r.x + 10.0, y, r.w - 20.0, 36.0);
                let selected = *m == self.module;
                if selected || self.hot(row) {
                    c.rounded(row, 10.0, Some(argb(fg(), if selected { 46 } else { 20 })), None);
                }
                c.icon(R::new(row.x + 8.0, row.y, 24.0, row.h), m.glyph(), 15.0, fg());
                c.text(R::new(row.x + 40.0, row.y, row.w - 48.0, row.h), &m.title(), 14.0, false, fg(), Align::Left);
                self.hit(row, Action::Go(*m));
                y += 38.0;
            }
        }
        c.text(R::new(r.x + 20.0, r.bottom() - 40.0, r.w - 40.0, 28.0), &format!("Engine {}", self.version), 12.0, false, dim(), Align::Left);
    }

    /// Large centred intro: title, text and checkmarked points left, the page's symbol right.
    fn draw_intro(&mut self, c: &mut Canvas, page: R, title: &str, subtitle: &str, points: &[String], glyph: char) {
        let cx = page.x + page.w / 2.0;
        let top = page.y + page.h * 0.24;
        let left = R::new(cx - 360.0, top, 360.0, 300.0);
        c.text(R::new(left.x, left.y, left.w, 44.0), title, 32.0, true, fg(), Align::Left);
        c.para(R::new(left.x, left.y + 54.0, left.w, 80.0), subtitle, 15.0, dim());
        let mut y = left.y + 140.0;
        for p in points {
            c.icon(R::new(left.x, y, 20.0, 22.0), CHECK, 13.0, mint());
            c.text(R::new(left.x + 28.0, y, left.w - 28.0, 22.0), p, 14.0, false, fg(), Align::Left);
            y += 30.0;
        }
        let (gx, gy) = (cx + 200.0, top + 110.0);
        c.circle(gx, gy, 96.0, Some(argb(fg(), 18)), Some((argb(fg(), 60), 1.0)));
        c.icon(R::new(gx - 80.0, gy - 80.0, 160.0, 160.0), glyph, 72.0, fg());
    }

    /// The big round call-to-action, centred on `cx`.
    fn big_button(&mut self, c: &mut Canvas, cx: f32, cy: f32, title: &str, busy: bool, action: Option<Action>) {
        let r = R::new(cx - 52.0, cy - 52.0, 104.0, 104.0);
        let enabled = action.is_some() && !busy;
        let hot = enabled && self.hot(r);
        c.circle(cx, cy, 52.0, Some(argb(fg(), if hot { 70 } else { 46 })), Some((argb(fg(), 90), 1.0)));
        c.circle(cx, cy, 58.0, None, Some((argb(fg(), if enabled || busy { 115 } else { 50 }), 2.0)));
        if busy {
            let t = self.started.elapsed().as_secs_f32();
            c.arc(cx, cy, 20.0, 3.0, (t * 360.0) % 360.0, 270.0, argb(fg(), 230));
        } else {
            c.text(r, title, 17.0, true, if enabled { fg() } else { dim() }, Align::Center);
        }
        if let (Some(a), true) = (action, enabled) {
            self.hit(r, a);
        }
    }

    /// Secondary glass pill.
    fn pill(&mut self, c: &mut Canvas, x: f32, cy: f32, title: &str, action: Action) -> f32 {
        let w = 40.0 + title.chars().count() as f32 * 7.6;
        let r = R::new(x, cy - 20.0, w, 40.0);
        c.rounded(r, 20.0, Some(argb(fg(), if self.hot(r) { 70 } else { 41 })), Some(argb(fg(), 60)));
        c.text(r, title, 14.0, true, fg(), Align::Center);
        self.hit(r, action);
        w
    }

    /// A pill that ends at `right`.
    fn pill_right(&mut self, c: &mut Canvas, right: f32, cy: f32, title: &str, action: Action) -> f32 {
        let w = 40.0 + title.chars().count() as f32 * 7.6;
        self.pill(c, right - w, cy, title, action);
        w
    }

    fn icon_pill(&mut self, c: &mut Canvas, x: f32, cy: f32, glyph: char, action: Action) {
        let r = R::new(x, cy - 20.0, 40.0, 40.0);
        c.rounded(r, 20.0, Some(argb(fg(), if self.hot(r) { 70 } else { 41 })), Some(argb(fg(), 60)));
        c.icon(r, glyph, 14.0, fg());
        self.hit(r, action);
    }

    fn checkbox(&self, c: &mut Canvas, x: f32, cy: f32, on: bool, enabled: bool) {
        if !enabled {
            c.circle(x + 9.0, cy, 9.0, None, Some((argb(fg(), 70), 1.5)));
        } else if on {
            c.circle(x + 9.0, cy, 10.0, Some(argb(fg(), 240)), None);
            c.icon(R::new(x, cy - 10.0, 18.0, 20.0), CHECK, 11.0, if light() { WHITE } else { 0x1B2A4A });
        } else {
            c.circle(x + 9.0, cy, 9.0, None, Some((argb(fg(), 200), 1.5)));
        }
    }

    /// A tick box that can also be partly ticked ("−": only some of what it holds goes).
    fn checkbox_mixed(&self, c: &mut Canvas, x: f32, cy: f32, on: bool, partial: bool) {
        if partial {
            c.circle(x + 9.0, cy, 10.0, Some(argb(fg(), 240)), None);
            c.icon(R::new(x, cy - 10.0, 18.0, 20.0), MINUS, 11.0, if light() { WHITE } else { 0x1B2A4A });
        } else {
            self.checkbox(c, x, cy, on, true);
        }
    }

    /// The chevron that shows or hides what a row holds.
    fn chevron(&mut self, c: &mut Canvas, r: R, open: bool, action: Action) {
        if self.hot(r) {
            c.rounded(r, 8.0, Some(argb(fg(), 30)), None);
        }
        c.icon(r, if open { OPENED } else { CHEVRON }, 11.0, dim());
        self.hit(r, action);
    }

    /// Whether `r` shows in the scroll panel being drawn (rows outside are only measured).
    fn visible(&self, r: R) -> bool {
        self.hit_clip.is_none_or(|v| r.intersects(&v))
    }

    /// The children of a folder opened in a list, each with its tick box; returns the y below.
    #[allow(clippy::too_many_arguments)]
    fn part_rows(&mut self, c: &mut Canvas, m: Module, x: f32, w: f32, mut yy: f32, parent: &str, parent_on: bool, act: impl Fn(String) -> Action) -> f32 {
        let Some(kids) = self.parts(m).children.get(parent).cloned() else {
            c.text(R::new(x + 12.0, yy, w - 24.0, 26.0), "Loading…", 12.0, false, dim(), Align::Left);
            return yy + 28.0;
        };
        if kids.is_empty() {
            c.text(R::new(x + 12.0, yy, w - 24.0, 26.0), "Empty", 12.0, false, dim(), Align::Left);
            return yy + 28.0;
        }
        for k in &kids {
            let path = k["path"].as_str().unwrap_or("").to_string();
            let row = R::new(x, yy, w, 28.0);
            yy += 30.0;
            let on = parent_on && self.parts(m).is_ticked(&path, parent);
            self.hit(row, act(path));
            if !self.visible(row) {
                continue;
            }
            if self.hot(row) {
                c.rounded(row, 10.0, Some(argb(fg(), 16)), None);
            }
            self.checkbox(c, row.x + 10.0, row.y + 14.0, on, true);
            c.icon(R::new(row.x + 36.0, row.y, 22.0, row.h), if k["is_dir"] == true { FOLDER } else { FILE }, 13.0, fg());
            c.text(R::new(row.x + 64.0, row.y, row.w - 190.0, row.h), k["name"].as_str().unwrap_or("?"), 12.0, false, fg(), Align::Left);
            c.text(R::new(row.right() - 150.0, row.y, 110.0, row.h), &bytes(k["size"].as_f64()), 12.0, false, dim(), Align::Right);
        }
        yy
    }

    /// How far a removal is: a bar over the items, the one being removed now, bytes freed.
    fn removal_progress(&self, c: &mut Canvas, r: R, p: &Value) {
        let done: f64 = ["removed", "skipped", "failed"].iter().filter_map(|k| p[*k].as_f64()).sum();
        let total = p["total"].as_f64().unwrap_or(1.0).max(1.0);
        let bar = R::new(r.x, r.y, r.w, 6.0);
        c.rounded(bar, 3.0, Some(argb(fg(), 38)), None);
        c.rounded(R::new(bar.x, bar.y, bar.w * (done.min(total) / total) as f32, 6.0), 3.0, Some(argb(fg(), 220)), None);
        let line = R::new(r.x, r.y + 12.0, r.w, 20.0);
        if let Some(current) = p["current"].as_str() {
            let name = std::path::Path::new(current).file_name().map_or(current.into(), |n| n.to_string_lossy());
            c.text(R::new(line.x, line.y, line.w * 0.6, line.h), &format!("Removing {name}"), 12.0, false, dim(), Align::Left);
        }
        let count = format!("{done:.0} of {total:.0} · {} freed", bytes(p["bytes"].as_f64()));
        c.text(R::new(line.x + line.w * 0.6, line.y, line.w * 0.4, line.h), &count, 12.0, false, dim(), Align::Right);
    }

    /// Header for pages with results: symbol, title, status line.
    fn header(&self, c: &mut Canvas, page: R, glyph: char, title: &str, status: &str) -> f32 {
        let y = page.y + 30.0;
        c.circle(page.x + 36.0, y + 36.0, 36.0, Some(argb(fg(), 18)), Some((argb(fg(), 60), 1.0)));
        c.icon(R::new(page.x, y, 72.0, 72.0), glyph, 30.0, fg());
        c.text(R::new(page.x + 90.0, y + 8.0, page.w - 90.0, 32.0), title, 24.0, true, fg(), Align::Left);
        c.text(R::new(page.x + 90.0, y + 40.0, page.w - 90.0, 22.0), status, 14.0, false, dim(), Align::Left);
        y + 90.0
    }

    /// A scrollable glass panel; `rows` draws its content at y offset and returns the height.
    fn scroll_panel(&mut self, c: &mut Canvas, r: R, scroll: f32, rows: impl FnOnce(&mut Self, &mut Canvas, f32) -> f32) {
        c.glass(r, 20.0);
        let inner = r.inset(8.0, 8.0);
        c.clip(inner);
        self.hit_clip = Some(inner);
        let height = rows(self, c, inner.y - scroll);
        self.hit_clip = None;
        c.unclip();
        self.view = Some((inner, height));
        if height > inner.h {
            let bar_h = (inner.h * inner.h / height).max(30.0);
            let bar_y = inner.y + (inner.h - bar_h) * (scroll / (height - inner.h)).clamp(0.0, 1.0);
            c.rounded(R::new(r.right() - 7.0, bar_y, 3.0, bar_h), 1.5, Some(argb(fg(), 90)), None);
        }
    }

    // ------------------------------------------------------------ dashboard

    fn draw_dashboard(&mut self, c: &mut Canvas, page: R) {
        let s = self.dash.status.clone();
        let x = page.x + 18.0;
        let top = page.y + 40.0;
        let tx = x;
        c.text(R::new(tx, top + 6.0, 300.0, 40.0), "Dashboard", 32.0, true, fg(), Align::Left);
        self.icon_pill(c, tx + 190.0, top + 26.0, REFRESH, Action::DashRefresh);
        let hw = &s["hardware"];
        let line: Vec<&str> = ["model", "cpu_model", "total_ram", "os_version"].iter().filter_map(|k| hw[*k].as_str()).filter(|v| !v.is_empty()).collect();
        c.text(R::new(tx, top + 56.0, page.w - 40.0, 20.0), &line.join(" · "), 13.0, false, dim(), Align::Left);
        if let Some(up) = s["uptime"].as_str() {
            c.text(R::new(tx, top + 78.0, page.w - 40.0, 20.0), &format!("Up for {up}"), 13.0, false, dim(), Align::Left);
        }

        // tiles
        let ty = top + 120.0;
        let tw = (page.w - 36.0 - 3.0 * 16.0) / 4.0;
        let disk = s["disks"].as_array().and_then(|d| d.first()).cloned().unwrap_or(Value::Null);
        let tiles: [(&str, String, Option<f64>, String); 4] = [
            ("CPU", pct(s["cpu"]["usage"].as_f64()), s["cpu"]["usage"].as_f64(), format!("{} cores", s["cpu"]["cores"].as_u64().unwrap_or(0))),
            (
                "Memory",
                pct(s["memory"]["used_percent"].as_f64()),
                s["memory"]["used_percent"].as_f64(),
                format!("{} / {}", bytes(s["memory"]["used"].as_f64()), bytes(s["memory"]["total"].as_f64())),
            ),
            (
                "Disk",
                pct(disk["used_percent"].as_f64()),
                disk["used_percent"].as_f64(),
                format!("{} free", bytes(disk["total"].as_f64().zip(disk["used"].as_f64()).map(|(t, u)| t - u))),
            ),
            ("Recycle Bin", bytes(s["trash_size"].as_f64()), None, String::new()),
        ];
        for (i, (title, value, fraction, detail)) in tiles.iter().enumerate() {
            let r = R::new(x + i as f32 * (tw + 16.0), ty, tw, 130.0);
            c.glass(r, 20.0);
            c.text(R::new(r.x + 18.0, r.y + 14.0, r.w - 36.0, 20.0), title, 13.0, false, dim(), Align::Left);
            c.text(R::new(r.x + 18.0, r.y + 38.0, r.w - 36.0, 36.0), value, 26.0, true, fg(), Align::Left);
            if let Some(f) = fraction {
                let bar = R::new(r.x + 18.0, r.y + 84.0, r.w - 36.0, 6.0);
                c.rounded(bar, 3.0, Some(argb(fg(), 38)), None);
                c.rounded(R::new(bar.x, bar.y, bar.w * (*f as f32 / 100.0).clamp(0.0, 1.0), 6.0), 3.0, Some(argb(amber(), 255)), None);
            }
            c.text(R::new(r.x + 18.0, r.y + 98.0, r.w - 36.0, 20.0), detail, 13.0, false, dim(), Align::Left);
        }

        // processes
        let pr = R::new(x, ty + 150.0, page.w - 36.0, (page.bottom() - ty - 170.0).max(80.0));
        c.glass(pr, 20.0);
        c.text(R::new(pr.x + 20.0, pr.y + 14.0, 300.0, 22.0), "Top processes", 15.0, true, fg(), Align::Left);
        let rows = ((pr.h - 52.0) / 26.0).floor().max(0.0) as usize;
        for (i, p) in s["top_processes"].as_array().into_iter().flatten().take(rows.min(8)).enumerate() {
            let y = pr.y + 44.0 + i as f32 * 26.0;
            c.text(R::new(pr.x + 20.0, y, pr.w * 0.6, 24.0), p["name"].as_str().unwrap_or("?"), 13.0, false, fg(), Align::Left);
            let right = format!("{} CPU · {}", pct(p["cpu"].as_f64()), bytes(p["memory_bytes"].as_f64()));
            c.text(R::new(pr.x + pr.w * 0.5, y, pr.w * 0.5 - 20.0, 24.0), &right, 13.0, false, dim(), Align::Right);
        }
    }

    // ------------------------------------------------------------ System Junk

    fn draw_junk(&mut self, c: &mut Canvas, page: R) {
        let tool = tool("clean");
        let (title, subtitle, points) = tool.map(|t| (t.title, t.subtitle, t.points)).unwrap_or_default();
        let bar_y = page.bottom() - 90.0;
        let cx = page.x + page.w / 2.0;
        match self.junk.phase {
            Phase::Idle => {
                self.draw_intro(c, page, &title, &subtitle, &points, Module::Junk.glyph());
                self.big_button(c, cx, bar_y, "Scan", false, Some(Action::JunkScan));
            }
            Phase::Finding | Phase::Review => {
                let found = bytes(self.junk.plan["bytes"].as_f64());
                let status = if self.junk.phase == Phase::Finding {
                    format!("Scanning… {found} found so far")
                } else {
                    format!("{found} found. Nothing has been removed yet.")
                };
                let y = self.header(c, page.inset(18.0, 0.0), Module::Junk.glyph(), &title, &status);
                let panel = R::new(page.x + 18.0, y, page.w - 36.0, bar_y - 72.0 - y);
                // the plan lists every item: borrowed for the paint rather than copied
                let plan = std::mem::take(&mut self.junk.plan);
                let review = self.junk.phase == Phase::Review;
                let scroll = self.junk.scroll;
                self.scroll_panel(c, panel, scroll, |st, c, mut yy| {
                    let start = yy;
                    if plan["groups"].as_array().is_none_or(|g| g.is_empty()) {
                        c.text(R::new(panel.x + 20.0, yy + 10.0, panel.w - 40.0, 24.0), "Looking through caches, logs and app data…", 14.0, false, dim(), Align::Left);
                        return 50.0;
                    }
                    for g in plan["groups"].as_array().into_iter().flatten() {
                        c.text(R::new(panel.x + 20.0, yy, panel.w - 40.0, 24.0), g["title"].as_str().unwrap_or(""), 13.0, true, dim(), Align::Left);
                        yy += 28.0;
                        for rule in g["rules"].as_array().into_iter().flatten() {
                            yy = st.junk_rule_row(c, panel, yy, rule, review);
                        }
                        yy += 10.0;
                    }
                    yy - start
                });
                self.junk.plan = plan;
                if self.junk.phase == Phase::Finding {
                    self.big_button(c, cx, bar_y, "", true, None);
                    self.pill(c, cx + 90.0, bar_y, "Stop", Action::JunkStop);
                } else {
                    let selected = self.junk_selected_bytes();
                    self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::JunkReset);
                    self.big_button(c, cx, bar_y, "Clean", false, (selected > 0.0).then_some(Action::JunkClean));
                    c.text(R::new(cx + 90.0, bar_y - 15.0, 200.0, 30.0), &bytes(Some(selected)), 17.0, true, fg(), Align::Left);
                }
            }
            Phase::Acting | Phase::Done => {
                let o = if self.junk.phase == Phase::Done { &self.junk.outcome } else { &self.junk.progress };
                let status = if self.junk.phase == Phase::Done {
                    format!("Done: {} freed", bytes(o["bytes"].as_f64()))
                } else {
                    format!("Cleaning… {} freed", bytes(o["bytes"].as_f64()))
                };
                let o = o.clone();
                let y = self.header(c, page.inset(18.0, 0.0), Module::Junk.glyph(), &title, &status);
                if self.junk.phase == Phase::Done {
                    self.outcome_panel(c, R::new(page.x + 18.0, y, page.w - 36.0, 120.0), &o);
                    self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::JunkReset);
                    self.big_button(c, cx, bar_y, "Scan", false, Some(Action::JunkScan));
                } else {
                    self.removal_progress(c, R::new(page.x + 18.0, y, page.w - 36.0, 40.0), &o);
                    self.big_button(c, cx, bar_y, "", true, None);
                }
            }
        }
    }

    /// One rule of the review: its tick box, what it is, its size and, once opened, its items
    /// (and the children of items opened in turn). Returns the y below.
    fn junk_rule_row(&mut self, c: &mut Canvas, panel: R, mut yy: f32, rule: &Value, review: bool) -> f32 {
        let id = rule["id"].as_str().unwrap_or("").to_string();
        let ready = review && rule["status"] == "ready";
        let in_use = rule["status"] == "in_use";
        let on = self.junk.selected.contains(&id);
        let items = rule_items(rule);
        let partial = on && items.iter().any(|(p, _)| self.junk.excluded.contains(p) || self.junk.parts.is_partial(p));
        let open = ready && !items.is_empty() && self.junk.expanded.contains(&id);
        let row = R::new(panel.x + 10.0, yy, panel.w - 20.0, 52.0);
        if ready {
            self.hit(row, Action::JunkToggle(id.clone()));
        }
        if self.visible(row) {
            if (on && ready) || (ready && self.hot(row)) {
                c.rounded(row, 12.0, Some(argb(fg(), if on { 31 } else { 16 })), None);
            }
            if ready {
                self.checkbox_mixed(c, row.x + 12.0, row.y + 26.0, on, partial);
            } else {
                self.checkbox(c, row.x + 12.0, row.y + 26.0, false, false);
            }
            c.text(R::new(row.x + 44.0, row.y + 6.0, row.w - 230.0, 22.0), rule["title"].as_str().unwrap_or(&id), 14.0, true, fg(), Align::Left);
            let (note, color) = match rule["in_use_by"].as_str() {
                Some(app) if in_use => (format!("Skipped while {app} is open"), amber()),
                _ => (rule["note"].as_str().unwrap_or("").to_string(), dim()),
            };
            c.text(R::new(row.x + 44.0, row.y + 28.0, row.w - 230.0, 18.0), &note, 12.0, false, color, Align::Left);
            if !in_use {
                c.text(R::new(row.right() - 176.0, row.y, 136.0, row.h), &bytes(rule["bytes"].as_f64()), 14.0, false, fg(), Align::Right);
            }
            if ready && !items.is_empty() {
                self.chevron(c, R::new(row.right() - 34.0, row.y + 14.0, 24.0, 24.0), open, Action::JunkExpand(id.clone()));
            }
        }
        yy += 56.0;
        if !open {
            return yy;
        }
        let x = row.x + 30.0;
        let w = row.w - 30.0;
        for (path, size) in items {
            let item_on = on && !self.junk.excluded.contains(&path);
            let item = R::new(x, yy, w, 30.0);
            yy += 32.0;
            self.hit(item, Action::JunkItem(id.clone(), path.clone()));
            // only a folder's chevron opens it
            let opened = self.junk.parts.open.contains(&path);
            if self.visible(item) {
                if self.hot(item) {
                    c.rounded(item, 10.0, Some(argb(fg(), 16)), None);
                }
                self.checkbox_mixed(c, item.x + 10.0, item.y + 15.0, item_on, item_on && self.junk.parts.is_partial(&path));
                // "folder/name": the name alone is often "Cache" or "logs"
                let p = std::path::Path::new(&path);
                let parent = p.parent().and_then(|q| q.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let name = p.file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                c.text(R::new(item.x + 38.0, item.y, item.w - 200.0, item.h), &format!("{parent}\\{name}"), 12.0, false, fg(), Align::Left);
                c.text(R::new(item.right() - 150.0, item.y, 110.0, item.h), &bytes(Some(self.junk.parts.bytes(&path, size))), 12.0, false, dim(), Align::Right);
                if self.junk.parts.can_open(&path) {
                    self.chevron(c, R::new(item.right() - 32.0, item.y + 3.0, 24.0, 24.0), opened, Action::JunkOpen(path.clone()));
                }
            }
            if opened {
                let (rid, item_path) = (id.clone(), path.clone());
                yy = self.part_rows(c, Module::Junk, x + 24.0, w - 24.0, yy, &path, item_on, move |child| Action::JunkPart(rid.clone(), item_path.clone(), child));
            }
        }
        yy + 6.0
    }

    /// The paths of a rule's items, for a rule by id.
    fn junk_items(&self, id: &str) -> Vec<String> {
        rules_of(&self.junk.plan).find(|r| r["id"] == id).map(|r| rule_items(r).into_iter().map(|(p, _)| p).collect()).unwrap_or_default()
    }

    /// Ticking a rule ticks all its items whole; a fully ticked rule unticks.
    fn junk_toggle_rule(&mut self, id: &str) {
        let items = self.junk_items(id);
        let j = &mut self.junk;
        let whole = items.iter().all(|p| !j.excluded.contains(p) && !j.parts.is_partial(p));
        if j.selected.contains(id) && whole {
            j.selected.remove(id);
        } else {
            j.selected.insert(id.to_string());
        }
        for p in &items {
            j.excluded.remove(p);
            j.parts.whole(p);
        }
    }

    fn junk_toggle_item(&mut self, id: &str, path: &str) {
        let items = self.junk_items(id);
        let j = &mut self.junk;
        if !j.selected.contains(id) {
            // ticking one item of an unticked rule: that item only
            j.selected.insert(id.to_string());
            j.excluded.extend(items.iter().filter(|p| *p != path).cloned());
        } else if !j.excluded.remove(path) {
            j.excluded.insert(path.to_string());
            if items.iter().all(|p| j.excluded.contains(p)) {
                j.selected.remove(id);
                for p in &items {
                    j.excluded.remove(p);
                }
            }
        }
        j.parts.whole(path);
    }

    fn junk_toggle_part(&mut self, id: &str, path: &str, child: &str) {
        let on = self.junk.selected.contains(id) && !self.junk.excluded.contains(path);
        if !on {
            self.junk_toggle_item(id, path);
            self.junk.parts.only(child, path);
        } else if !self.junk.parts.toggle(child, path) {
            self.junk_toggle_item(id, path);
        }
    }

    fn junk_selected_bytes(&self) -> f64 {
        let j = &self.junk;
        rules_of(&j.plan)
            .filter(|r| r["status"] == "ready" && r["id"].as_str().is_some_and(|id| j.selected.contains(id)))
            .map(|r| {
                let items = rule_items(r);
                if items.is_empty() {
                    return r["bytes"].as_f64().unwrap_or(0.0);
                }
                items.iter().filter(|(p, _)| !j.excluded.contains(p)).map(|(p, b)| j.parts.bytes(p, *b)).sum()
            })
            .sum()
    }

    fn outcome_panel(&self, c: &mut Canvas, r: R, o: &Value) {
        c.glass(r, 20.0);
        let mut y = r.y + 18.0;
        if let Some(e) = o["error"].as_str() {
            c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), e, 15.0, true, amber(), Align::Left);
            y += 26.0;
        }
        let line = format!("{} items removed, {} freed", o["removed"].as_u64().unwrap_or(0), bytes(o["bytes"].as_f64()));
        c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), &line, 15.0, true, fg(), Align::Left);
        y += 26.0;
        if o["skipped"].as_u64().unwrap_or(0) > 0 {
            let s = format!("{} items left alone: in use or changed since the scan", o["skipped"]);
            c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), &s, 14.0, false, dim(), Align::Left);
            y += 24.0;
        }
        if o["failed"].as_u64().unwrap_or(0) > 0 {
            let s = format!("{} items could not be removed", o["failed"]);
            c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), &s, 14.0, false, amber(), Align::Left);
        }
    }

    // ------------------------------------------------------------ pick lists

    fn draw_picks(&mut self, c: &mut Canvas, page: R, m: Module) {
        let (title, subtitle, points) = m.intro();
        let bar_y = page.bottom() - 90.0;
        let cx = page.x + page.w / 2.0;
        let phase = self.pick(m).phase;
        match phase {
            Phase::Idle => {
                self.draw_intro(c, page, &title, &subtitle, &points, m.glyph());
                self.big_button(c, cx, bar_y, "Scan", false, Some(Action::PickFind(m)));
            }
            Phase::Finding => {
                self.header(c, page.inset(18.0, 0.0), m.glyph(), &title, "Looking…");
                self.big_button(c, cx, bar_y, "", true, None);
                self.pill(c, cx + 90.0, bar_y, "Stop", Action::PickStop(m));
            }
            Phase::Review | Phase::Acting => {
                let review = phase == Phase::Review;
                let p = self.pick(m);
                let count = items_of(&p.list).count();
                let status = if review {
                    format!("{count} found, {}. Nothing has been removed yet.", bytes(p.list["bytes"].as_f64()))
                } else {
                    "Working…".into()
                };
                let mut y = self.header(c, page.inset(18.0, 0.0), m.glyph(), &title, &status);
                if !review {
                    let progress = p.progress.clone();
                    self.removal_progress(c, R::new(page.x + 18.0, y, page.w - 36.0, 40.0), &progress);
                    y += 44.0;
                }
                let panel = R::new(page.x + 18.0, y, page.w - 36.0, bar_y - 72.0 - y);
                self.pick_list(c, panel, m, review);
                if review {
                    let selected = self.picks_selected_bytes(m);
                    let any = !self.pick(m).selected.is_empty();
                    self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::PickReset(m));
                    self.big_button(c, cx, bar_y, if m.trashes() { "Remove" } else { "Delete" }, false, any.then_some(Action::PickAct(m)));
                    c.text(R::new(cx + 90.0, bar_y - 15.0, 200.0, 30.0), &bytes(Some(selected)), 17.0, true, fg(), Align::Left);
                } else {
                    self.big_button(c, cx, bar_y, "", true, None);
                }
            }
            Phase::Done => {
                let o = self.pick(m).outcome.clone();
                let status = format!("Done: {} freed", bytes(o["bytes"].as_f64()));
                let y = self.header(c, page.inset(18.0, 0.0), m.glyph(), &title, &status);
                self.outcome_panel(c, R::new(page.x + 18.0, y, page.w - 36.0, 120.0), &o);
                self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::PickReset(m));
                self.big_button(c, cx, bar_y, "Scan", false, Some(Action::PickFind(m)));
            }
        }
    }

    /// The tick list of a pick list, grouped; folders open to tick some of their children.
    /// While acting it only shows.
    fn pick_list(&mut self, c: &mut Canvas, panel: R, m: Module, review: bool) {
        // borrowed for the paint rather than copied
        let list = std::mem::take(&mut self.picks(m).list);
        let scroll = self.pick(m).scroll;
        self.scroll_panel(c, panel, scroll, |st, c, mut yy| {
            let start = yy;
            let mut last_group = "";
            let items: Vec<&Value> = items_of(&list).collect();
            if items.is_empty() {
                c.text(R::new(panel.x + 20.0, yy + 10.0, 300.0, 24.0), "Nothing found.", 14.0, false, dim(), Align::Left);
                return 50.0;
            }
            for (i, it) in items.into_iter().enumerate() {
                let group = it["group"].as_str().unwrap_or("");
                if group != last_group && !group.is_empty() {
                    c.text(R::new(panel.x + 20.0, yy, panel.w - 40.0, 24.0), group, 13.0, true, dim(), Align::Left);
                    yy += 28.0;
                }
                last_group = group;
                let path = it["path"].as_str().unwrap_or("").to_string();
                let on = st.pick(m).selected.contains(&i);
                let opened = review && st.pick(m).parts.open.contains(&path);
                let row = R::new(panel.x + 10.0, yy, panel.w - 20.0, 44.0);
                yy += 48.0;
                if review {
                    st.hit(row, Action::PickToggle(m, i));
                }
                if st.visible(row) {
                    let parts = &mut st.picks(m).parts;
                    let is_dir = parts.can_open(&path);
                    let partial = on && parts.is_partial(&path);
                    let size = parts.bytes(&path, it["bytes"].as_f64().unwrap_or(0.0));
                    if on || (review && st.hot(row)) {
                        c.rounded(row, 12.0, Some(argb(fg(), if on { 31 } else { 16 })), None);
                    }
                    st.checkbox_mixed(c, row.x + 12.0, row.y + 22.0, on, partial);
                    c.icon(R::new(row.x + 40.0, row.y, 24.0, row.h), if is_dir { FOLDER } else { FILE }, 16.0, fg());
                    c.text(R::new(row.x + 72.0, row.y + 3.0, row.w - 260.0, 20.0), it["name"].as_str().unwrap_or("?"), 14.0, true, fg(), Align::Left);
                    c.text(R::new(row.x + 72.0, row.y + 23.0, row.w - 260.0, 18.0), it["detail"].as_str().unwrap_or(""), 12.0, false, dim(), Align::Left);
                    c.text(R::new(row.right() - 176.0, row.y, 136.0, row.h), &bytes(Some(size)), 14.0, false, fg(), Align::Right);
                    if review && is_dir {
                        st.chevron(c, R::new(row.right() - 34.0, row.y + 10.0, 24.0, 24.0), opened, Action::PickOpen(m, i));
                    }
                }
                if opened {
                    yy = st.part_rows(c, m, row.x + 40.0, row.w - 40.0, yy, &path, on, move |child| Action::PickPart(m, i, child));
                }
            }
            yy - start
        });
        self.picks(m).list = list;
    }

    fn picks_selected_bytes(&self, m: Module) -> f64 {
        let p = self.pick(m);
        let items: Vec<&Value> = items_of(&p.list).collect();
        p.selected
            .iter()
            .filter_map(|&i| items.get(i))
            .map(|it| p.parts.bytes(it["path"].as_str().unwrap_or(""), it["bytes"].as_f64().unwrap_or(0.0)))
            .sum()
    }

    fn pick_path(&self, m: Module, i: usize) -> String {
        items_of(&self.pick(m).list).nth(i).and_then(|it| it["path"].as_str()).unwrap_or("").to_string()
    }

    // ------------------------------------------------------------ Uninstaller

    fn draw_apps(&mut self, c: &mut Canvas, page: R) {
        let bar_y = page.bottom() - 90.0;
        let cx = page.x + page.w / 2.0;
        let status = self.apps.message.clone().unwrap_or_else(|| "Start an app's own uninstaller. Apps that need no installer can also be removed in Settings.".into());
        let y = self.header(c, page.inset(18.0, 0.0), Module::Uninstaller.glyph(), "Uninstaller", &status);
        self.icon_pill(c, page.right() - 58.0, page.y + 66.0, REFRESH, Action::AppsReload);
        let panel = R::new(page.x + 18.0, y, page.w - 36.0, bar_y - 72.0 - y);
        if !self.apps.loaded {
            c.glass(panel, 20.0);
            let t = self.started.elapsed().as_secs_f32();
            c.arc(panel.x + panel.w / 2.0, panel.y + panel.h / 2.0, 18.0, 3.0, (t * 360.0) % 360.0, 270.0, argb(fg(), 230));
            return;
        }
        let mut order: Vec<usize> = (0..self.apps.list.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.app_size(i)));
        let scroll = self.apps.scroll;
        self.scroll_panel(c, panel, scroll, |st, c, mut yy| {
            let start = yy;
            for i in order {
                let app = st.apps.list[i].clone();
                let key = app["bundle_id"].as_str().unwrap_or("").to_string();
                let on = st.apps.chosen.contains(&key);
                let row = R::new(panel.x + 10.0, yy, panel.w - 20.0, 48.0);
                if on || st.hot(row) {
                    c.rounded(row, 12.0, Some(argb(fg(), if on { 31 } else { 16 })), None);
                }
                st.checkbox(c, row.x + 12.0, row.y + 24.0, on, true);
                c.text(R::new(row.x + 44.0, row.y + 4.0, row.w - 200.0, 22.0), app["name"].as_str().unwrap_or("?"), 14.0, true, fg(), Align::Left);
                let detail: Vec<&str> = [app["publisher"].as_str(), app["version"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
                c.text(R::new(row.x + 44.0, row.y + 26.0, row.w - 200.0, 18.0), &detail.join(" · "), 12.0, false, dim(), Align::Left);
                let size = st.app_size(i);
                c.text(R::new(row.right() - 150.0, row.y, 136.0, row.h), &if size > 0 { bytes(Some(size as f64)) } else { "…".into() }, 14.0, false, fg(), Align::Right);
                // registered even when scrolled out: the keyboard moves along every row
                st.hit(row, Action::AppToggle(key));
                yy += 52.0;
            }
            yy - start
        });
        let any = !self.apps.chosen.is_empty();
        self.big_button(c, cx, bar_y, "Uninstall", false, any.then_some(Action::AppsUninstall));
    }

    fn app_size(&self, i: usize) -> u64 {
        let measured = self.apps.sizes.get(i).copied().unwrap_or(0);
        if measured > 0 { measured } else { self.apps.list[i]["estimated_size"].as_u64().unwrap_or(0) }
    }

    // ------------------------------------------------------------ Space Lens

    fn draw_space(&mut self, c: &mut Canvas, page: R) {
        if self.space.stack.is_empty() {
            self.draw_intro(
                c,
                R::new(page.x, page.y - 90.0, page.w, page.h),
                "Space Lens",
                "See what takes up space as an interactive map, then send what you don't need to the Recycle Bin.",
                &[],
                Module::SpaceLens.glyph(),
            );
            let cards = [
                (None, "This PC", "Your files, apps and Windows at a glance"),
                (std::env::var("USERPROFILE").ok(), "Your user folder", "Everything in your profile, folder by folder"),
            ];
            let cw = 230.0;
            let x0 = page.x + page.w / 2.0 - cw - 8.0;
            for (i, (path, title, text)) in cards.into_iter().enumerate() {
                let r = R::new(x0 + i as f32 * (cw + 16.0), page.y + page.h * 0.56, cw, 170.0);
                c.rounded(r, 24.0, Some(argb(fg(), if self.hot(r) { 40 } else { 26 })), Some(argb(fg(), 56)));
                c.icon(R::new(r.x, r.y + 16.0, r.w, 56.0), if path.is_none() { DRIVE } else { FOLDER }, 40.0, fg());
                c.text(R::new(r.x + 12.0, r.y + 82.0, r.w - 24.0, 24.0), title, 15.0, true, fg(), Align::Center);
                c.text(R::new(r.x + 12.0, r.y + 108.0, r.w - 24.0, 40.0), text, 12.0, false, dim(), Align::Center);
                self.hit(r, Action::SpaceOpen(path));
            }
            return;
        }
        let key = self.space.stack.last().cloned().flatten();
        let k = key.clone().unwrap_or_default();
        let listing = self.space.listings.get(&k).cloned();
        let bar_y = page.bottom() - 90.0;
        let cx = page.x + page.w / 2.0;
        let top = page.y + 36.0;
        // header: back, where, refresh, start over
        self.icon_pill(c, page.x + 18.0, top + 20.0, BACK, Action::SpaceBack);
        let place = key.clone().unwrap_or_else(|| "This PC".into());
        c.text(R::new(page.x + 70.0, top, page.w - 330.0, 40.0), &place, 15.0, true, fg(), Align::Left);
        let w = self.pill_right(c, page.right() - 18.0, top + 20.0, "Start Over", Action::SpaceStartOver);
        self.icon_pill(c, page.right() - 18.0 - w - 50.0, top + 20.0, REFRESH, Action::SpaceRefresh);

        let body = R::new(page.x + 18.0, top + 56.0, page.w - 36.0, bar_y - 72.0 - (top + 56.0));
        let Some(listing) = listing else {
            c.glass(body, 20.0);
            if let Some(e) = &self.space.error {
                c.text(body, e, 14.0, false, amber(), Align::Center);
            } else {
                let t = self.started.elapsed().as_secs_f32();
                c.arc(body.x + body.w / 2.0, body.y + body.h / 2.0 - 20.0, 22.0, 3.0, (t * 360.0) % 360.0, 270.0, argb(fg(), 230));
                c.text(R::new(body.x, body.y + body.h / 2.0 + 16.0, body.w, 24.0), "Mapping your files…", 14.0, false, dim(), Align::Center);
            }
            return;
        };
        let entries: Vec<Value> = listing["entries"].as_array().cloned().unwrap_or_default();
        let list_r = R::new(body.x, body.y, 420.0_f32.min(body.w * 0.48), body.h);
        let scroll = self.space.scroll;
        let live = self.space.job.as_ref().is_some_and(|(jk, _)| *jk == k);
        self.scroll_panel(c, list_r, scroll, |st, c, mut yy| {
            let start = yy;
            for e in &entries {
                let path = e["path"].as_str().unwrap_or("").to_string();
                let selectable = e["selectable"] == true && !live;
                let on = st.space.selected.contains(&path);
                let row = R::new(list_r.x + 8.0, yy, list_r.w - 16.0, 46.0);
                if on || st.hot(row) {
                    c.rounded(row, 12.0, Some(argb(fg(), if on { 31 } else { 16 })), None);
                }
                if selectable {
                    st.checkbox(c, row.x + 10.0, row.y + 23.0, on, true);
                    st.hit(R::new(row.x, row.y, 34.0, row.h), Action::SpaceToggle(path.clone()));
                }
                let is_dir = e["is_dir"] == true;
                c.icon(R::new(row.x + 36.0, row.y, 26.0, row.h), if is_dir { FOLDER } else { FILE }, 17.0, fg());
                c.text(R::new(row.x + 68.0, row.y + 4.0, row.w - 190.0, 20.0), e["name"].as_str().unwrap_or("?"), 14.0, true, fg(), Align::Left);
                if e["scan_status"] == "partial" {
                    c.text(R::new(row.x + 68.0, row.y + 24.0, row.w - 190.0, 18.0), "Partly measured", 12.0, false, dim(), Align::Left);
                }
                c.text(R::new(row.right() - 130.0, row.y, 96.0, row.h), &bytes(e["size"].as_f64()), 14.0, false, fg(), Align::Right);
                if is_dir {
                    c.icon(R::new(row.right() - 30.0, row.y, 24.0, row.h), CHEVRON, 11.0, dim());
                    if row.intersects(&list_r) {
                        st.hit(R::new(row.x + 34.0, row.y, row.w - 34.0, row.h), Action::SpaceOpen(Some(path.clone())));
                    }
                } else if row.intersects(&list_r) {
                    st.hit(R::new(row.x + 34.0, row.y, row.w - 34.0, row.h), Action::Reveal(path.clone()));
                }
                yy += 50.0;
            }
            yy - start
        });
        let chart = R::new(list_r.right() + 24.0, body.y, body.right() - list_r.right() - 24.0, body.h);
        self.draw_bubbles(c, chart, &entries);
        if live {
            c.text(R::new(chart.x, chart.bottom() - 24.0, chart.w, 20.0), "Measuring… sizes keep growing until the scan finishes.", 12.0, false, dim(), Align::Center);
        }

        // bottom: drive usage left, Remove centre, selected size right
        let selected_bytes: f64 = entries.iter().filter(|e| e["path"].as_str().is_some_and(|p| self.space.selected.contains(p))).filter_map(|e| e["size"].as_f64()).sum();
        if !self.space.selected.is_empty() {
            let busy = self.space.remove_job.is_some();
            self.big_button(c, cx, bar_y, "Remove", busy, Some(Action::SpaceRemove));
            c.text(R::new(cx + 90.0, bar_y - 15.0, 200.0, 30.0), &bytes(Some(selected_bytes)), 17.0, true, fg(), Align::Left);
        }
        if let Some(e) = &self.space.error {
            c.text(R::new(page.x + 18.0, bar_y - 12.0, cx - page.x - 120.0, 24.0), e, 13.0, false, amber(), Align::Left);
        }
    }

    fn draw_bubbles(&mut self, c: &mut Canvas, r: R, entries: &[Value]) {
        let sized: Vec<&Value> = entries.iter().filter(|e| e["size"].as_f64().unwrap_or(0.0) > 0.0).collect();
        let total: f64 = sized.iter().filter_map(|e| e["size"].as_f64()).sum();
        if total <= 0.0 {
            return;
        }
        // the 14 biggest that are at least 1.5% each, the rest as one "Others" bubble
        let big: Vec<&Value> = sized.iter().take(14).filter(|e| e["size"].as_f64().unwrap_or(0.0) >= total * 0.015).copied().collect();
        let mut sizes: Vec<f64> = big.iter().filter_map(|e| e["size"].as_f64()).collect();
        let rest = total - sizes.iter().sum::<f64>();
        if rest >= total * 0.015 {
            sizes.push(rest);
        }
        let side = r.w.min(r.h);
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        c.circle(cx, cy, side / 2.0, Some(argb(fg(), 22)), Some((argb(fg(), 50), 1.0)));
        let tints = [0x5EE6D0, 0x4FC3F7, 0x7C9CFF, 0xB388FF, 0x64B5F6];
        for (rank, (i, bx, by, br)) in pack(&sizes, side / 2.0 * 0.9).into_iter().enumerate() {
            let (x, y) = (cx + bx, cy + by);
            let entry = big.get(i);
            let tint = if entry.is_some() { tints[rank % tints.len()] } else { fg() };
            let hit = R::new(x - br, y - br, 2.0 * br, 2.0 * br);
            let hot = self.hot(hit);
            c.circle(x, y, br, Some(argb(tint, if hot { 120 } else { 80 })), Some((argb(fg(), if hot { 150 } else { 80 }), 1.0)));
            let name = entry.and_then(|e| e["name"].as_str()).unwrap_or("Others");
            if br >= 48.0 {
                c.text(R::new(x - br * 0.75, y - 18.0, br * 1.5, 20.0), name, 13.0, true, fg(), Align::Center);
                c.text(R::new(x - br * 0.75, y + 2.0, br * 1.5, 18.0), &bytes(Some(sizes[i])), 12.0, false, dim(), Align::Center);
            }
            if let Some(e) = entry
                && e["is_dir"] == true
                && let Some(p) = e["path"].as_str()
            {
                self.hit(hit, Action::SpaceOpen(Some(p.to_string())));
            }
        }
    }

    fn space_load(&mut self, path: Option<String>) {
        let key = path.clone().unwrap_or_default();
        if let Some((_, j)) = self.space.job.take() {
            j.cancel();
        }
        let mut req = json!({"op": "space"});
        if let Some(p) = &path {
            req["path"] = json!(p);
        }
        self.space.error = None;
        self.space.job = Some((key, jobs::start(self.hwnd, req)));
        self.update_timers();
    }

    // ------------------------------------------------------------ History

    fn draw_history(&mut self, c: &mut Canvas, page: R) {
        let y = self.header(c, page.inset(18.0, 0.0), Module::History.glyph(), "History", "What OpenCleaner removed, newest first.");
        let bottom = page.bottom() - 40.0;
        let panel = R::new(page.x + 18.0, y, page.w - 36.0, bottom - y);
        let sessions = self.history.sessions.clone();
        let scroll = self.history.scroll;
        self.scroll_panel(c, panel, scroll, |_, c, mut yy| {
            let start = yy;
            if sessions.is_empty() {
                c.text(R::new(panel.x + 20.0, yy + 10.0, 400.0, 24.0), "Nothing yet.", 14.0, false, dim(), Align::Left);
                return 50.0;
            }
            for s in &sessions {
                let title = match s["command"].as_str().unwrap_or("") {
                    "clean" => Module::Junk.title(),
                    "installers" => Module::Installers.title(),
                    "projects" => Module::Projects.title(),
                    "large_old" => Module::LargeOld.title(),
                    "duplicates" => Module::Duplicates.title(),
                    "uninstall" => "Uninstaller".into(),
                    other => other.to_string(),
                };
                c.text(R::new(panel.x + 20.0, yy + 4.0, panel.w * 0.6, 20.0), &title, 14.0, true, fg(), Align::Left);
                c.text(R::new(panel.x + 20.0, yy + 24.0, panel.w * 0.6, 18.0), &when(s["started_unix"].as_u64().unwrap_or(0)), 12.0, false, dim(), Align::Left);
                let right = format!("{} items · {}", s["items"].as_u64().unwrap_or(0), bytes(s["bytes"].as_f64()));
                c.text(R::new(panel.x + panel.w * 0.5, yy, panel.w * 0.5 - 20.0, 46.0), &right, 14.0, false, fg(), Align::Right);
                yy += 50.0;
            }
            yy - start
        });
        if let Some(log) = &self.history.log {
            c.text(R::new(page.x + 18.0, bottom + 8.0, page.w - 36.0, 20.0), &format!("Every deleted file is listed in {log}"), 12.0, false, dim(), Align::Left);
        }
    }

    // ------------------------------------------------------------ Settings

    fn draw_settings(&mut self, c: &mut Canvas, page: R) {
        let y = self.header(c, page.inset(18.0, 0.0), Module::Settings.glyph(), "Settings", "Light or dark follows the Windows app theme.");
        let panel = R::new(page.x + 18.0, y, page.w - 36.0, 168.0);
        c.glass(panel, 20.0);
        c.text(R::new(panel.x + 20.0, panel.y + 12.0, 200.0, 20.0), "Engine", 13.0, true, dim(), Align::Left);
        let row = R::new(panel.x + 10.0, panel.y + 38.0, panel.w - 20.0, 36.0);
        if self.hot(row) {
            c.rounded(row, 10.0, Some(argb(fg(), 16)), None);
        }
        c.text(R::new(row.x + 10.0, row.y, row.w - 100.0, row.h), "Faster scans", 15.0, true, fg(), Align::Left);
        // a switch: track and knob
        let track = R::new(row.right() - 58.0, row.y + 7.0, 44.0, 22.0);
        c.rounded(track, 11.0, Some(if self.fast { argb(mint(), 255) } else { argb(fg(), 50) }), None);
        let knob_x = if self.fast { track.right() - 11.0 } else { track.x + 11.0 };
        c.circle(knob_x, track.y + 11.0, 8.0, Some(argb(WHITE, 255)), None);
        self.hit(row, Action::FastToggle);
        c.para(
            R::new(panel.x + 20.0, row.bottom() + 8.0, panel.w - 40.0, 54.0),
            "Off (recommended): scans and removals give way to your other apps, so the PC stays smooth. On: they take every CPU core to finish sooner, and other apps may lag or freeze until they are done.",
            13.0,
            dim(),
        );
        c.text(R::new(panel.x + 20.0, panel.bottom() - 30.0, panel.w - 40.0, 20.0), &format!("OpenCleaner engine {}", self.version), 12.0, false, dim(), Align::Left);
    }
}

// ---------------------------------------------------------------- performing actions

/// Runs an action. Confirmations happen outside the state borrow (the dialog has its own
/// message loop that paints this window).
pub fn perform(hwnd: HWND, action: Action) {
    use super::with;
    match action {
        Action::Go(m) => {
            with(|st| {
                st.module = m;
                st.on_show_module();
            });
        }
        Action::DashRefresh => {
            with(|st| st.refresh_dashboard());
        }
        Action::SpaceOpen(path) => {
            with(|st| {
                st.space.stack.push(path.clone());
                st.space.selected.clear();
                st.space.scroll = 0.0;
                if !st.space.listings.contains_key(&path.clone().unwrap_or_default()) {
                    st.space_load(path);
                }
            });
        }
        Action::SpaceBack => {
            with(|st| {
                st.space.stack.pop();
                st.space.selected.clear();
                st.space.scroll = 0.0;
            });
        }
        Action::SpaceStartOver => {
            with(|st| {
                st.space.stack.clear();
                st.space.selected.clear();
            });
        }
        Action::SpaceRefresh => {
            with(|st| {
                let current = st.space.stack.last().cloned().flatten();
                st.space.listings.remove(&current.clone().unwrap_or_default());
                st.space_load(current);
            });
        }
        Action::SpaceToggle(p) => {
            with(|st| {
                if !st.space.selected.remove(&p) {
                    st.space.selected.insert(p);
                }
            });
        }
        Action::SpaceRemove => {
            let Some((n, size)) = with(|st| {
                let k = st.space.stack.last().cloned().flatten().unwrap_or_default();
                let size: f64 = st.space.listings.get(&k).and_then(|l| l["entries"].as_array()).into_iter().flatten()
                    .filter(|e| e["path"].as_str().is_some_and(|p| st.space.selected.contains(p)))
                    .filter_map(|e| e["size"].as_f64())
                    .sum();
                (st.space.selected.len(), size)
            }) else { return };
            if confirm(hwnd, &format!("Move {n} items ({}) to the Recycle Bin?", bytes(Some(size)))) {
                with(|st| {
                    let paths: Vec<String> = st.space.selected.iter().cloned().collect();
                    st.space.remove_job = Some(jobs::start(hwnd, json!({"op": "trash_paths", "paths": paths})));
                    st.update_timers();
                });
            }
        }
        Action::Reveal(p) => reveal(&p),
        Action::JunkScan => {
            with(|st| {
                if let Some(j) = &st.junk.job {
                    j.cancel();
                }
                st.junk = Junk { phase: Phase::Finding, ..Junk::default() };
                // list: every rule's items, so they can be opened and unticked one by one
                st.junk.job = Some(jobs::start(hwnd, json!({"op": "scan", "list": true})));
                st.update_timers();
            });
        }
        Action::JunkStop => {
            with(|st| {
                if let Some(j) = &st.junk.job {
                    j.cancel();
                }
            });
        }
        Action::JunkToggle(id) => {
            with(|st| st.junk_toggle_rule(&id));
        }
        Action::JunkExpand(id) => {
            with(|st| {
                if !st.junk.expanded.remove(&id) {
                    st.junk.expanded.insert(id);
                }
            });
        }
        Action::JunkItem(id, path) => {
            with(|st| st.junk_toggle_item(&id, &path));
        }
        Action::JunkOpen(path) => {
            with(|st| st.junk.parts.toggle_open(&path, hwnd));
        }
        Action::JunkPart(id, path, child) => {
            with(|st| st.junk_toggle_part(&id, &path, &child));
        }
        Action::JunkClean => {
            let Some(size) = with(|st| st.junk_selected_bytes()) else { return };
            if confirm(hwnd, &format!("Remove {} of junk? This cannot be undone.", bytes(Some(size)))) {
                with(|st| {
                    let j = &st.junk;
                    let plan = j.plan["plan"].as_u64().unwrap_or(0);
                    let rules: Vec<&String> = j.selected.iter().collect();
                    let skip: Vec<&String> = j.excluded.iter().collect();
                    // items going, for the folders opened with only some children ticked
                    let chosen: HashSet<String> = rules_of(&j.plan)
                        .filter(|r| r["id"].as_str().is_some_and(|id| j.selected.contains(id)))
                        .flat_map(|r| rule_items(r).into_iter().map(|(p, _)| p))
                        .filter(|p| !j.excluded.contains(p))
                        .collect();
                    let parts = j.parts.request(|p| chosen.contains(p));
                    let request = json!({"op": "clean", "plan": plan, "rules": rules, "skip": skip, "parts": parts});
                    st.junk.phase = Phase::Acting;
                    st.junk.progress = Value::Null;
                    st.junk.job = Some(jobs::start(hwnd, request));
                    st.update_timers();
                });
            }
        }
        Action::JunkReset => {
            with(|st| st.junk = Junk::default());
        }
        Action::PickFind(m) => {
            with(|st| {
                if let Some(j) = &st.pick(m).job {
                    j.cancel();
                }
                let job = jobs::start(hwnd, m.find_request());
                *st.picks(m) = Picks { phase: Phase::Finding, job: Some(job), ..Picks::default() };
                st.update_timers();
            });
        }
        Action::PickStop(m) => {
            with(|st| {
                if let Some(j) = &st.picks(m).job {
                    j.cancel();
                }
            });
        }
        Action::PickToggle(m, i) => {
            with(|st| {
                let path = st.pick_path(m, i);
                let p = st.picks(m);
                if !p.selected.remove(&i) {
                    p.selected.insert(i);
                }
                p.parts.whole(&path);
            });
        }
        Action::PickOpen(m, i) => {
            with(|st| {
                let path = st.pick_path(m, i);
                st.picks(m).parts.toggle_open(&path, hwnd);
            });
        }
        Action::PickPart(m, i, child) => {
            with(|st| {
                let path = st.pick_path(m, i);
                let p = st.picks(m);
                if !p.selected.contains(&i) {
                    // ticking a child of an unticked folder: that child only
                    p.selected.insert(i);
                    p.parts.only(&child, &path);
                } else if !p.parts.toggle(&child, &path) {
                    p.selected.remove(&i);
                }
            });
        }
        Action::PickAct(m) => {
            let Some((n, size)) = with(|st| (st.pick(m).selected.len(), st.picks_selected_bytes(m))) else { return };
            let text = if m.trashes() {
                format!("Move {n} items ({}) to the Recycle Bin?", bytes(Some(size)))
            } else {
                format!("Delete {n} build folders ({})? They are rebuilt the next time you build.", bytes(Some(size)))
            };
            if confirm(hwnd, &text) {
                with(|st| {
                    let p = st.picks(m);
                    let plan = p.list["plan"].as_u64().unwrap_or(0);
                    let mut items: Vec<usize> = p.selected.iter().copied().collect();
                    items.sort_unstable();
                    // folders opened with only some children ticked: those children, by index
                    let parts: HashMap<String, Vec<&String>> = items
                        .iter()
                        .filter_map(|&i| {
                            let path = items_of(&p.list).nth(i)?["path"].as_str()?;
                            Some((i.to_string(), p.parts.ticked.get(path)?.iter().collect()))
                        })
                        .collect();
                    let request = json!({"op": "remove", "plan": plan, "items": items, "parts": parts});
                    p.phase = Phase::Acting;
                    p.progress = Value::Null;
                    p.job = Some(jobs::start(hwnd, request));
                    st.update_timers();
                });
            }
        }
        Action::PickReset(m) => {
            with(|st| *st.picks(m) = Picks::default());
        }
        Action::AppToggle(key) => {
            with(|st| {
                if !st.apps.chosen.remove(&key) {
                    st.apps.chosen.insert(key);
                }
            });
        }
        Action::FastToggle => {
            with(|st| {
                st.fast = set_fast(!st.fast);
                super::set_reg_dword(super::SETTINGS_KEY, FAST_VALUE, st.fast as u32);
            });
        }
        Action::AppsReload => {
            with(|st| {
                st.apps.message = None;
                st.load_apps();
            });
        }
        Action::AppsUninstall => {
            let Some(chosen) = with(|st| {
                st.apps
                    .list
                    .iter()
                    .filter(|a| a["bundle_id"].as_str().is_some_and(|k| st.apps.chosen.contains(k)))
                    .map(|a| (a["bundle_id"].as_str().unwrap_or("").to_string(), a["name"].as_str().unwrap_or("").to_string()))
                    .collect::<Vec<_>>()
            }) else { return };
            let names: Vec<&str> = chosen.iter().map(|(_, n)| n.as_str()).collect();
            if !confirm(hwnd, &format!("Start the uninstaller of {}? Each opens its own window.", names.join(", "))) {
                return;
            }
            let mut started = Vec::new();
            let mut failed = Vec::new();
            for (key, name) in &chosen {
                let r = jobs::call(json!({"op": "run_uninstaller", "key": key}));
                if r["ok"] == true { started.push(name.clone()) } else { failed.push(format!("{name}: {}", r["error"].as_str().unwrap_or("?"))) }
            }
            with(|st| {
                st.apps.chosen.clear();
                st.apps.message = Some(if failed.is_empty() {
                    format!("Started: {}. Press refresh when they are done.", started.join(", "))
                } else {
                    format!("Could not start {}", failed.join("; "))
                });
            });
        }
    }
    invalidate(hwnd);
}

// ---------------------------------------------------------------- helpers

fn rules_of(plan: &Value) -> impl Iterator<Item = &Value> {
    plan["groups"].as_array().into_iter().flatten().flat_map(|g| g["rules"].as_array().into_iter().flatten())
}

/// The items a rule found, biggest first (as the engine lists them): (path, bytes).
fn rule_items(rule: &Value) -> Vec<(String, f64)> {
    rule["paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| Some((pair[0].as_str()?.to_string(), pair[1].as_f64().unwrap_or(0.0))))
        .collect()
}

fn items_of(list: &Value) -> impl Iterator<Item = &Value> {
    list["items"].as_array().into_iter().flatten()
}

/// Decimal units, like Finder and Explorer's "size" column.
pub fn bytes(n: Option<f64>) -> String {
    let Some(n) = n else { return "—".into() };
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n.max(0.0);
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    match u {
        0 => format!("{v:.0} {}", units[u]),
        1 => format!("{v:.0} {}", units[u]),
        _ => format!("{v:.1} {}", units[u]),
    }
}

fn pct(n: Option<f64>) -> String {
    n.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into())
}

/// Local date and time of a Unix timestamp, "2026-11-18 21:04".
fn when(unix: u64) -> String {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows_sys::Win32::System::Time::FileTimeToSystemTime;
    let ticks = (unix + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut local: FILETIME = unsafe { std::mem::zeroed() };
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        FileTimeToLocalFileTime(&ft, &mut local);
        FileTimeToSystemTime(&local, &mut st);
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute)
}

/// Area-proportional circles, largest first, each placed at the first free spot along an
/// outward spiral, then scaled to fit `radius` (the same layout as the macOS map).
fn pack(sizes: &[f64], radius: f32) -> Vec<(usize, f32, f32, f32)> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].total_cmp(&sizes[a]));
    let max = sizes.iter().cloned().fold(1.0, f64::max);
    let mut placed: Vec<(usize, f32, f32, f32)> = Vec::new();
    for i in order {
        let r = ((sizes[i] / max).sqrt() as f32).max(0.06);
        let (mut angle, mut p) = (0.0f32, (0.0f32, 0.0f32));
        while placed.iter().any(|&(_, x, y, pr)| ((x - p.0).powi(2) + (y - p.1).powi(2)).sqrt() < pr + r + 0.03) {
            angle += 0.12;
            let d = angle * 0.04;
            p = (angle.cos() * d, angle.sin() * d);
        }
        placed.push((i, p.0, p.1, r));
    }
    if placed.is_empty() {
        return placed;
    }
    let min_x = placed.iter().map(|b| b.1 - b.3).fold(f32::MAX, f32::min);
    let max_x = placed.iter().map(|b| b.1 + b.3).fold(f32::MIN, f32::max);
    let min_y = placed.iter().map(|b| b.2 - b.3).fold(f32::MAX, f32::min);
    let max_y = placed.iter().map(|b| b.2 + b.3).fold(f32::MIN, f32::max);
    let (mx, my) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    let extent = placed.iter().map(|b| ((b.1 - mx).powi(2) + (b.2 - my).powi(2)).sqrt() + b.3).fold(0.0, f32::max).max(1e-3);
    let k = radius / extent;
    placed.into_iter().map(|(i, x, y, r)| (i, (x - mx) * k, (y - my) * k, r * k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tick rules of an opened folder, as on macOS (Parts.swift).
    #[test]
    fn parts_ticks() {
        let mut p = Parts::default();
        let kids = |names: &[&str]| names.iter().map(|n| json!({"path": n, "size": 10})).collect::<Vec<_>>();
        p.children.insert("D".into(), kids(&["a", "b", "c"]));
        assert_eq!(p.bytes("D", 99.0), 99.0, "whole folder: its own size");
        assert!(p.toggle("a", "D"));
        assert!(p.is_partial("D") && !p.is_ticked("a", "D") && p.is_ticked("b", "D"));
        assert_eq!(p.bytes("D", 99.0), 20.0);
        assert!(p.toggle("a", "D"));
        assert!(!p.is_partial("D"), "all ticked again: whole");
        p.only("b", "D");
        assert!(!p.toggle("b", "D"), "none left: the caller unticks the folder");
        assert!(!p.is_partial("D"));
        p.only("c", "D");
        assert_eq!(p.request(|k| k == "D")["D"], vec!["c".to_string()]);
    }
}
