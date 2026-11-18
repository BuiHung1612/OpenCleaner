//! Pages, drawn the way the macOS app lays them out: a glass sidebar on a gradient that
//! belongs to the page, content on glass panels, the big round button centred at the
//! bottom with pill buttons beside it. Each paint registers the areas that act on a click.

use super::gfx::{AMBER, Align, Canvas, DIM, Fonts, MINT, R, WHITE, argb, hex};
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
    History,
}

/// Sidebar: dashboard, then groups, then history (as on macOS).
const SECTIONS: &[(&str, &[Module])] = &[
    ("", &[Module::Dashboard]),
    ("Cleanup", &[Module::Junk, Module::Installers, Module::Projects]),
    ("Applications", &[Module::Uninstaller]),
    ("Files", &[Module::SpaceLens]),
    (" ", &[Module::History]),
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
            Module::History => "History",
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
            Module::History => '\u{E81C}',
        }
    }
}

const CHECK: char = '\u{E73E}';
const BACK: char = '\u{E72B}';
const REFRESH: char = '\u{E72C}';
const CHEVRON: char = '\u{E76C}';
const FOLDER: char = '\u{E8B7}';
const FILE: char = '\u{E7C3}';
const DRIVE: char = '\u{EDA2}';

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
    JunkClean,
    JunkReset,
    PickFind(Module),
    PickStop(Module),
    PickToggle(Module, usize),
    PickAct(Module),
    PickReset(Module),
    AppToggle(String),
    AppsUninstall,
    AppsReload,
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

#[derive(Default)]
struct Junk {
    phase: Phase,
    plan: Value,
    selected: HashSet<String>,
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
    apps: Apps,
    history: History,
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
            apps: Apps::default(),
            history: History::default(),
        }
    }

    fn picks(&mut self, m: Module) -> &mut Picks {
        if m == Module::Installers { &mut self.installers } else { &mut self.projects }
    }

    fn busy(&self) -> bool {
        self.dash.job.is_some()
            || self.space.job.is_some()
            || self.space.remove_job.is_some()
            || self.junk.job.is_some()
            || self.installers.job.is_some()
            || self.projects.job.is_some()
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
            Module::Installers => &mut self.installers.scroll,
            Module::Projects => &mut self.projects.scroll,
            Module::Uninstaller => &mut self.apps.scroll,
            Module::History => &mut self.history.scroll,
            Module::Dashboard => return,
        };
        *s = (*s + dy).clamp(0.0, max);
    }

    fn hit(&mut self, r: R, a: Action) {
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
        self.update_timers();
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
        } else if is(&self.installers.job) || is(&self.projects.job) {
            let m = if is(&self.installers.job) { Module::Installers } else { Module::Projects };
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
        let (from, to) = self.module.palette();
        c.gradient(R::new(0.0, 0.0, w, h), from, to);
        // two soft light blobs, so the glass has something to sit on
        c.circle(w * 0.85, h * 0.08, 360.0, Some(argb(from, 70)), None);
        c.circle(w * 0.2, h * 0.98, 300.0, Some(argb(WHITE, 14)), None);

        self.draw_sidebar(c, R::new(14.0, 14.0, 236.0, h - 28.0));
        let page = R::new(264.0, 0.0, w - 264.0 - 14.0, h);
        match self.module {
            Module::Dashboard => self.draw_dashboard(c, page),
            Module::Junk => self.draw_junk(c, page),
            Module::Installers | Module::Projects => self.draw_picks(c, page, self.module),
            Module::Uninstaller => self.draw_apps(c, page),
            Module::SpaceLens => self.draw_space(c, page),
            Module::History => self.draw_history(c, page),
        }
    }

    fn draw_sidebar(&mut self, c: &mut Canvas, r: R) {
        c.glass(r, 22.0);
        let mut y = r.y + 10.0;
        for (group, modules) in SECTIONS {
            if !group.trim().is_empty() {
                c.text(R::new(r.x + 22.0, y + 8.0, r.w - 30.0, 20.0), group, 12.0, false, DIM, Align::Left);
                y += 30.0;
            } else if *group == " " {
                y += 12.0;
            }
            for m in modules.iter() {
                let row = R::new(r.x + 10.0, y, r.w - 20.0, 36.0);
                let selected = *m == self.module;
                if selected || self.hot(row) {
                    c.rounded(row, 10.0, Some(argb(WHITE, if selected { 46 } else { 20 })), None);
                }
                c.icon(R::new(row.x + 8.0, row.y, 24.0, row.h), m.glyph(), 15.0, WHITE);
                c.text(R::new(row.x + 40.0, row.y, row.w - 48.0, row.h), &m.title(), 14.0, false, WHITE, Align::Left);
                self.hit(row, Action::Go(*m));
                y += 38.0;
            }
        }
        c.text(R::new(r.x + 20.0, r.bottom() - 40.0, r.w - 40.0, 28.0), &format!("Engine {}", self.version), 12.0, false, DIM, Align::Left);
    }

    /// Large centred intro: title, text and checkmarked points left, the page's symbol right.
    fn draw_intro(&mut self, c: &mut Canvas, page: R, title: &str, subtitle: &str, points: &[String], glyph: char) {
        let cx = page.x + page.w / 2.0;
        let top = page.y + page.h * 0.24;
        let left = R::new(cx - 360.0, top, 360.0, 300.0);
        c.text(R::new(left.x, left.y, left.w, 44.0), title, 32.0, true, WHITE, Align::Left);
        c.para(R::new(left.x, left.y + 54.0, left.w, 80.0), subtitle, 15.0, DIM);
        let mut y = left.y + 140.0;
        for p in points {
            c.icon(R::new(left.x, y, 20.0, 22.0), CHECK, 13.0, MINT);
            c.text(R::new(left.x + 28.0, y, left.w - 28.0, 22.0), p, 14.0, false, WHITE, Align::Left);
            y += 30.0;
        }
        let (gx, gy) = (cx + 200.0, top + 110.0);
        c.circle(gx, gy, 96.0, Some(argb(WHITE, 18)), Some((argb(WHITE, 60), 1.0)));
        c.icon(R::new(gx - 80.0, gy - 80.0, 160.0, 160.0), glyph, 72.0, WHITE);
    }

    /// The big round call-to-action, centred on `cx`.
    fn big_button(&mut self, c: &mut Canvas, cx: f32, cy: f32, title: &str, busy: bool, action: Option<Action>) {
        let r = R::new(cx - 52.0, cy - 52.0, 104.0, 104.0);
        let enabled = action.is_some() && !busy;
        let hot = enabled && self.hot(r);
        c.circle(cx, cy, 52.0, Some(argb(WHITE, if hot { 70 } else { 46 })), Some((argb(WHITE, 90), 1.0)));
        c.circle(cx, cy, 58.0, None, Some((argb(WHITE, if enabled || busy { 115 } else { 50 }), 2.0)));
        if busy {
            let t = self.started.elapsed().as_secs_f32();
            c.arc(cx, cy, 20.0, 3.0, (t * 360.0) % 360.0, 270.0, argb(WHITE, 230));
        } else {
            c.text(r, title, 17.0, true, if enabled { WHITE } else { DIM }, Align::Center);
        }
        if let (Some(a), true) = (action, enabled) {
            self.hit(r, a);
        }
    }

    /// Secondary glass pill.
    fn pill(&mut self, c: &mut Canvas, x: f32, cy: f32, title: &str, action: Action) -> f32 {
        let w = 40.0 + title.chars().count() as f32 * 7.6;
        let r = R::new(x, cy - 20.0, w, 40.0);
        c.rounded(r, 20.0, Some(argb(WHITE, if self.hot(r) { 70 } else { 41 })), Some(argb(WHITE, 60)));
        c.text(r, title, 14.0, true, WHITE, Align::Center);
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
        c.rounded(r, 20.0, Some(argb(WHITE, if self.hot(r) { 70 } else { 41 })), Some(argb(WHITE, 60)));
        c.icon(r, glyph, 14.0, WHITE);
        self.hit(r, action);
    }

    fn checkbox(&self, c: &mut Canvas, x: f32, cy: f32, on: bool, enabled: bool) {
        if !enabled {
            c.circle(x + 9.0, cy, 9.0, None, Some((argb(WHITE, 70), 1.5)));
        } else if on {
            c.circle(x + 9.0, cy, 10.0, Some(argb(WHITE, 240)), None);
            c.icon(R::new(x, cy - 10.0, 18.0, 20.0), CHECK, 11.0, 0x1B2A4A);
        } else {
            c.circle(x + 9.0, cy, 9.0, None, Some((argb(WHITE, 200), 1.5)));
        }
    }

    /// Header for pages with results: symbol, title, status line.
    fn header(&self, c: &mut Canvas, page: R, glyph: char, title: &str, status: &str) -> f32 {
        let y = page.y + 30.0;
        c.circle(page.x + 36.0, y + 36.0, 36.0, Some(argb(WHITE, 18)), Some((argb(WHITE, 60), 1.0)));
        c.icon(R::new(page.x, y, 72.0, 72.0), glyph, 30.0, WHITE);
        c.text(R::new(page.x + 90.0, y + 8.0, page.w - 90.0, 32.0), title, 24.0, true, WHITE, Align::Left);
        c.text(R::new(page.x + 90.0, y + 40.0, page.w - 90.0, 22.0), status, 14.0, false, DIM, Align::Left);
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
            c.rounded(R::new(r.right() - 7.0, bar_y, 3.0, bar_h), 1.5, Some(argb(WHITE, 90)), None);
        }
    }

    // ------------------------------------------------------------ dashboard

    fn draw_dashboard(&mut self, c: &mut Canvas, page: R) {
        let s = self.dash.status.clone();
        let x = page.x + 18.0;
        let top = page.y + 40.0;
        // health ring
        let (rx, ry) = (x + 82.0, top + 82.0);
        c.circle(rx, ry, 82.0, Some(argb(WHITE, 18)), Some((argb(WHITE, 50), 1.0)));
        c.arc(rx, ry, 64.0, 14.0, 0.0, 360.0, argb(WHITE, 30));
        if let Some(score) = s["health_score"].as_f64() {
            let color = if score >= 75.0 { MINT } else if score >= 50.0 { AMBER } else { 0xFF8A65 };
            c.arc(rx, ry, 64.0, 14.0, -90.0, (score / 100.0 * 360.0) as f32, argb(color, 255));
            c.text(R::new(rx - 60.0, ry - 30.0, 120.0, 46.0), &format!("{score:.0}"), 40.0, true, WHITE, Align::Center);
        } else {
            c.text(R::new(rx - 60.0, ry - 30.0, 120.0, 46.0), "—", 40.0, true, WHITE, Align::Center);
        }
        c.text(R::new(rx - 60.0, ry + 16.0, 120.0, 20.0), "Health", 13.0, false, DIM, Align::Center);

        let tx = x + 200.0;
        c.text(R::new(tx, top + 6.0, 300.0, 40.0), "Dashboard", 32.0, true, WHITE, Align::Left);
        self.icon_pill(c, tx + 190.0, top + 26.0, REFRESH, Action::DashRefresh);
        let msg = s["health_score_msg"].as_str().unwrap_or("Measuring system status…");
        c.text(R::new(tx, top + 56.0, page.w - 220.0, 22.0), msg, 15.0, false, DIM, Align::Left);
        let hw = &s["hardware"];
        let line: Vec<&str> = ["model", "cpu_model", "total_ram", "os_version"].iter().filter_map(|k| hw[*k].as_str()).filter(|v| !v.is_empty()).collect();
        c.text(R::new(tx, top + 82.0, page.w - 220.0, 20.0), &line.join(" · "), 13.0, false, DIM, Align::Left);
        if let Some(up) = s["uptime"].as_str() {
            c.text(R::new(tx, top + 104.0, page.w - 220.0, 20.0), &format!("Up for {up}"), 13.0, false, DIM, Align::Left);
        }

        // tiles
        let ty = top + 190.0;
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
            c.text(R::new(r.x + 18.0, r.y + 14.0, r.w - 36.0, 20.0), title, 13.0, false, DIM, Align::Left);
            c.text(R::new(r.x + 18.0, r.y + 38.0, r.w - 36.0, 36.0), value, 26.0, true, WHITE, Align::Left);
            if let Some(f) = fraction {
                let bar = R::new(r.x + 18.0, r.y + 84.0, r.w - 36.0, 6.0);
                c.rounded(bar, 3.0, Some(argb(WHITE, 38)), None);
                c.rounded(R::new(bar.x, bar.y, bar.w * (*f as f32 / 100.0).clamp(0.0, 1.0), 6.0), 3.0, Some(argb(AMBER, 255)), None);
            }
            c.text(R::new(r.x + 18.0, r.y + 98.0, r.w - 36.0, 20.0), detail, 13.0, false, DIM, Align::Left);
        }

        // processes
        let pr = R::new(x, ty + 150.0, page.w - 36.0, (page.bottom() - ty - 170.0).max(80.0));
        c.glass(pr, 20.0);
        c.text(R::new(pr.x + 20.0, pr.y + 14.0, 300.0, 22.0), "Top processes", 15.0, true, WHITE, Align::Left);
        let rows = ((pr.h - 52.0) / 26.0).floor().max(0.0) as usize;
        for (i, p) in s["top_processes"].as_array().into_iter().flatten().take(rows.min(8)).enumerate() {
            let y = pr.y + 44.0 + i as f32 * 26.0;
            c.text(R::new(pr.x + 20.0, y, pr.w * 0.6, 24.0), p["name"].as_str().unwrap_or("?"), 13.0, false, WHITE, Align::Left);
            let right = format!("{} CPU · {}", pct(p["cpu"].as_f64()), bytes(p["memory_bytes"].as_f64()));
            c.text(R::new(pr.x + pr.w * 0.5, y, pr.w * 0.5 - 20.0, 24.0), &right, 13.0, false, DIM, Align::Right);
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
                let plan = self.junk.plan.clone();
                let review = self.junk.phase == Phase::Review;
                let scroll = self.junk.scroll;
                self.scroll_panel(c, panel, scroll, |st, c, mut yy| {
                    let start = yy;
                    for g in plan["groups"].as_array().into_iter().flatten() {
                        c.text(R::new(panel.x + 20.0, yy, panel.w - 40.0, 24.0), g["title"].as_str().unwrap_or(""), 13.0, true, DIM, Align::Left);
                        yy += 28.0;
                        for rule in g["rules"].as_array().into_iter().flatten() {
                            let id = rule["id"].as_str().unwrap_or("").to_string();
                            let ready = review && rule["status"] == "ready";
                            let on = st.junk.selected.contains(&id);
                            let row = R::new(panel.x + 10.0, yy, panel.w - 20.0, 52.0);
                            if (on && ready) || (ready && st.hot(row)) {
                                c.rounded(row, 12.0, Some(argb(WHITE, if on { 31 } else { 16 })), None);
                            }
                            st.checkbox(c, row.x + 12.0, row.y + 26.0, on && ready, ready);
                            c.text(R::new(row.x + 44.0, row.y + 6.0, row.w - 200.0, 22.0), rule["title"].as_str().unwrap_or(&id), 14.0, true, WHITE, Align::Left);
                            let (note, color) = match rule["in_use_by"].as_str() {
                                Some(app) if rule["status"] == "in_use" => (format!("Skipped while {app} is open"), AMBER),
                                _ => (rule["note"].as_str().unwrap_or("").to_string(), DIM),
                            };
                            c.text(R::new(row.x + 44.0, row.y + 28.0, row.w - 200.0, 18.0), &note, 12.0, false, color, Align::Left);
                            c.text(R::new(row.right() - 150.0, row.y, 136.0, row.h), &bytes(rule["bytes"].as_f64()), 14.0, false, WHITE, Align::Right);
                            if ready && row.intersects(&panel) {
                                st.hit(row, Action::JunkToggle(id));
                            }
                            yy += 56.0;
                        }
                        yy += 10.0;
                    }
                    yy - start
                });
                if self.junk.phase == Phase::Finding {
                    self.big_button(c, cx, bar_y, "", true, None);
                    self.pill(c, cx + 90.0, bar_y, "Stop", Action::JunkStop);
                } else {
                    let selected = self.junk_selected_bytes();
                    self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::JunkReset);
                    self.big_button(c, cx, bar_y, "Clean", false, (selected > 0.0).then_some(Action::JunkClean));
                    c.text(R::new(cx + 90.0, bar_y - 15.0, 200.0, 30.0), &bytes(Some(selected)), 17.0, true, WHITE, Align::Left);
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
                    self.big_button(c, cx, bar_y, "", true, None);
                }
            }
        }
    }

    fn junk_selected_bytes(&self) -> f64 {
        rules_of(&self.junk.plan)
            .filter(|r| r["status"] == "ready" && r["id"].as_str().is_some_and(|id| self.junk.selected.contains(id)))
            .filter_map(|r| r["bytes"].as_f64())
            .sum()
    }

    fn outcome_panel(&self, c: &mut Canvas, r: R, o: &Value) {
        c.glass(r, 20.0);
        let mut y = r.y + 18.0;
        if let Some(e) = o["error"].as_str() {
            c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), e, 15.0, true, AMBER, Align::Left);
            y += 26.0;
        }
        let line = format!("{} items removed, {} freed", o["removed"].as_u64().unwrap_or(0), bytes(o["bytes"].as_f64()));
        c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), &line, 15.0, true, WHITE, Align::Left);
        y += 26.0;
        if o["skipped"].as_u64().unwrap_or(0) > 0 {
            let s = format!("{} items left alone: in use or changed since the scan", o["skipped"]);
            c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), &s, 14.0, false, DIM, Align::Left);
            y += 24.0;
        }
        if o["failed"].as_u64().unwrap_or(0) > 0 {
            let s = format!("{} items could not be removed", o["failed"]);
            c.text(R::new(r.x + 20.0, y, r.w - 40.0, 22.0), &s, 14.0, false, AMBER, Align::Left);
        }
    }

    // ------------------------------------------------------------ Installers / Project Junk

    fn draw_picks(&mut self, c: &mut Canvas, page: R, m: Module) {
        let t = m.tool_id().and_then(tool);
        let (title, subtitle, points) = t.map(|t| (t.title, t.subtitle, t.points)).unwrap_or_default();
        let bar_y = page.bottom() - 90.0;
        let cx = page.x + page.w / 2.0;
        let trashes = m == Module::Installers;
        let phase = self.picks(m).phase;
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
            Phase::Review => {
                let list = self.picks(m).list.clone();
                let items: Vec<Value> = items_of(&list).cloned().collect();
                let status = format!("{} found, {}. Nothing has been removed yet.", items.len(), bytes(list["bytes"].as_f64()));
                let y = self.header(c, page.inset(18.0, 0.0), m.glyph(), &title, &status);
                let panel = R::new(page.x + 18.0, y, page.w - 36.0, bar_y - 72.0 - y);
                let scroll = self.picks(m).scroll;
                self.scroll_panel(c, panel, scroll, |st, c, mut yy| {
                    let start = yy;
                    let mut last_group = String::new();
                    if items.is_empty() {
                        c.text(R::new(panel.x + 20.0, yy + 10.0, 300.0, 24.0), "Nothing found.", 14.0, false, DIM, Align::Left);
                        return 50.0;
                    }
                    for (i, it) in items.iter().enumerate() {
                        let group = it["group"].as_str().unwrap_or("").to_string();
                        if group != last_group && !group.is_empty() {
                            c.text(R::new(panel.x + 20.0, yy, panel.w - 40.0, 24.0), &group, 13.0, true, DIM, Align::Left);
                            yy += 28.0;
                        }
                        last_group = group;
                        let on = st.picks(m).selected.contains(&i);
                        let row = R::new(panel.x + 10.0, yy, panel.w - 20.0, 44.0);
                        if on || st.hot(row) {
                            c.rounded(row, 12.0, Some(argb(WHITE, if on { 31 } else { 16 })), None);
                        }
                        st.checkbox(c, row.x + 12.0, row.y + 22.0, on, true);
                        c.icon(R::new(row.x + 40.0, row.y, 24.0, row.h), if trashes { FILE } else { FOLDER }, 16.0, WHITE);
                        c.text(R::new(row.x + 72.0, row.y + 3.0, row.w - 230.0, 20.0), it["name"].as_str().unwrap_or("?"), 14.0, true, WHITE, Align::Left);
                        c.text(R::new(row.x + 72.0, row.y + 23.0, row.w - 230.0, 18.0), it["detail"].as_str().unwrap_or(""), 12.0, false, DIM, Align::Left);
                        c.text(R::new(row.right() - 150.0, row.y, 136.0, row.h), &bytes(it["bytes"].as_f64()), 14.0, false, WHITE, Align::Right);
                        if row.intersects(&panel) {
                            st.hit(row, Action::PickToggle(m, i));
                        }
                        yy += 48.0;
                    }
                    yy - start
                });
                let p = self.picks(m);
                let selected: f64 = p.selected.iter().filter_map(|&i| items.get(i)).filter_map(|it| it["bytes"].as_f64()).sum();
                let any = !p.selected.is_empty();
                self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::PickReset(m));
                self.big_button(c, cx, bar_y, if trashes { "Remove" } else { "Delete" }, false, any.then_some(Action::PickAct(m)));
                c.text(R::new(cx + 90.0, bar_y - 15.0, 200.0, 30.0), &bytes(Some(selected)), 17.0, true, WHITE, Align::Left);
            }
            Phase::Acting | Phase::Done => {
                let done = phase == Phase::Done;
                let o = if done { self.picks(m).outcome.clone() } else { self.picks(m).progress.clone() };
                let status = if done { format!("Done: {} freed", bytes(o["bytes"].as_f64())) } else { "Working…".into() };
                let y = self.header(c, page.inset(18.0, 0.0), m.glyph(), &title, &status);
                if done {
                    self.outcome_panel(c, R::new(page.x + 18.0, y, page.w - 36.0, 120.0), &o);
                    self.pill_right(c, cx - 90.0, bar_y, "Start Over", Action::PickReset(m));
                    self.big_button(c, cx, bar_y, "Scan", false, Some(Action::PickFind(m)));
                } else {
                    self.big_button(c, cx, bar_y, "", true, None);
                }
            }
        }
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
            c.arc(panel.x + panel.w / 2.0, panel.y + panel.h / 2.0, 18.0, 3.0, (t * 360.0) % 360.0, 270.0, argb(WHITE, 230));
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
                    c.rounded(row, 12.0, Some(argb(WHITE, if on { 31 } else { 16 })), None);
                }
                st.checkbox(c, row.x + 12.0, row.y + 24.0, on, true);
                c.text(R::new(row.x + 44.0, row.y + 4.0, row.w - 200.0, 22.0), app["name"].as_str().unwrap_or("?"), 14.0, true, WHITE, Align::Left);
                let detail: Vec<&str> = [app["publisher"].as_str(), app["version"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
                c.text(R::new(row.x + 44.0, row.y + 26.0, row.w - 200.0, 18.0), &detail.join(" · "), 12.0, false, DIM, Align::Left);
                let size = st.app_size(i);
                c.text(R::new(row.right() - 150.0, row.y, 136.0, row.h), &if size > 0 { bytes(Some(size as f64)) } else { "…".into() }, 14.0, false, WHITE, Align::Right);
                if row.intersects(&panel) {
                    st.hit(row, Action::AppToggle(key));
                }
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
                c.rounded(r, 24.0, Some(argb(WHITE, if self.hot(r) { 40 } else { 26 })), Some(argb(WHITE, 56)));
                c.icon(R::new(r.x, r.y + 16.0, r.w, 56.0), if path.is_none() { DRIVE } else { FOLDER }, 40.0, WHITE);
                c.text(R::new(r.x + 12.0, r.y + 82.0, r.w - 24.0, 24.0), title, 15.0, true, WHITE, Align::Center);
                c.text(R::new(r.x + 12.0, r.y + 108.0, r.w - 24.0, 40.0), text, 12.0, false, DIM, Align::Center);
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
        c.text(R::new(page.x + 70.0, top, page.w - 330.0, 40.0), &place, 15.0, true, WHITE, Align::Left);
        let w = self.pill_right(c, page.right() - 18.0, top + 20.0, "Start Over", Action::SpaceStartOver);
        self.icon_pill(c, page.right() - 18.0 - w - 50.0, top + 20.0, REFRESH, Action::SpaceRefresh);

        let body = R::new(page.x + 18.0, top + 56.0, page.w - 36.0, bar_y - 72.0 - (top + 56.0));
        let Some(listing) = listing else {
            c.glass(body, 20.0);
            if let Some(e) = &self.space.error {
                c.text(body, e, 14.0, false, AMBER, Align::Center);
            } else {
                let t = self.started.elapsed().as_secs_f32();
                c.arc(body.x + body.w / 2.0, body.y + body.h / 2.0 - 20.0, 22.0, 3.0, (t * 360.0) % 360.0, 270.0, argb(WHITE, 230));
                c.text(R::new(body.x, body.y + body.h / 2.0 + 16.0, body.w, 24.0), "Mapping your files…", 14.0, false, DIM, Align::Center);
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
                    c.rounded(row, 12.0, Some(argb(WHITE, if on { 31 } else { 16 })), None);
                }
                if selectable {
                    st.checkbox(c, row.x + 10.0, row.y + 23.0, on, true);
                    if row.intersects(&list_r) {
                        st.hit(R::new(row.x, row.y, 34.0, row.h), Action::SpaceToggle(path.clone()));
                    }
                }
                let is_dir = e["is_dir"] == true;
                c.icon(R::new(row.x + 36.0, row.y, 26.0, row.h), if is_dir { FOLDER } else { FILE }, 17.0, WHITE);
                c.text(R::new(row.x + 68.0, row.y + 4.0, row.w - 190.0, 20.0), e["name"].as_str().unwrap_or("?"), 14.0, true, WHITE, Align::Left);
                if e["scan_status"] == "partial" {
                    c.text(R::new(row.x + 68.0, row.y + 24.0, row.w - 190.0, 18.0), "Partly measured", 12.0, false, DIM, Align::Left);
                }
                c.text(R::new(row.right() - 130.0, row.y, 96.0, row.h), &bytes(e["size"].as_f64()), 14.0, false, WHITE, Align::Right);
                if is_dir {
                    c.icon(R::new(row.right() - 30.0, row.y, 24.0, row.h), CHEVRON, 11.0, DIM);
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
            c.text(R::new(chart.x, chart.bottom() - 24.0, chart.w, 20.0), "Measuring… sizes keep growing until the scan finishes.", 12.0, false, DIM, Align::Center);
        }

        // bottom: drive usage left, Remove centre, selected size right
        let selected_bytes: f64 = entries.iter().filter(|e| e["path"].as_str().is_some_and(|p| self.space.selected.contains(p))).filter_map(|e| e["size"].as_f64()).sum();
        if !self.space.selected.is_empty() {
            let busy = self.space.remove_job.is_some();
            self.big_button(c, cx, bar_y, "Remove", busy, Some(Action::SpaceRemove));
            c.text(R::new(cx + 90.0, bar_y - 15.0, 200.0, 30.0), &bytes(Some(selected_bytes)), 17.0, true, WHITE, Align::Left);
        }
        if let Some(e) = &self.space.error {
            c.text(R::new(page.x + 18.0, bar_y - 12.0, cx - page.x - 120.0, 24.0), e, 13.0, false, AMBER, Align::Left);
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
        c.circle(cx, cy, side / 2.0, Some(argb(WHITE, 22)), Some((argb(WHITE, 50), 1.0)));
        let tints = [0x5EE6D0, 0x4FC3F7, 0x7C9CFF, 0xB388FF, 0x64B5F6];
        for (rank, (i, bx, by, br)) in pack(&sizes, side / 2.0 * 0.9).into_iter().enumerate() {
            let (x, y) = (cx + bx, cy + by);
            let entry = big.get(i);
            let tint = if entry.is_some() { tints[rank % tints.len()] } else { WHITE };
            let hit = R::new(x - br, y - br, 2.0 * br, 2.0 * br);
            let hot = self.hot(hit);
            c.circle(x, y, br, Some(argb(tint, if hot { 120 } else { 80 })), Some((argb(WHITE, if hot { 150 } else { 80 }), 1.0)));
            let name = entry.and_then(|e| e["name"].as_str()).unwrap_or("Others");
            if br >= 48.0 {
                c.text(R::new(x - br * 0.75, y - 18.0, br * 1.5, 20.0), name, 13.0, true, WHITE, Align::Center);
                c.text(R::new(x - br * 0.75, y + 2.0, br * 1.5, 18.0), &bytes(Some(sizes[i])), 12.0, false, DIM, Align::Center);
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
                c.text(R::new(panel.x + 20.0, yy + 10.0, 400.0, 24.0), "Nothing yet.", 14.0, false, DIM, Align::Left);
                return 50.0;
            }
            for s in &sessions {
                let title = match s["command"].as_str().unwrap_or("") {
                    "clean" => Module::Junk.title(),
                    "installers" => Module::Installers.title(),
                    "projects" => Module::Projects.title(),
                    "uninstall" => "Uninstaller".into(),
                    other => other.to_string(),
                };
                c.text(R::new(panel.x + 20.0, yy + 4.0, panel.w * 0.6, 20.0), &title, 14.0, true, WHITE, Align::Left);
                c.text(R::new(panel.x + 20.0, yy + 24.0, panel.w * 0.6, 18.0), &when(s["started_unix"].as_u64().unwrap_or(0)), 12.0, false, DIM, Align::Left);
                let right = format!("{} items · {}", s["items"].as_u64().unwrap_or(0), bytes(s["bytes"].as_f64()));
                c.text(R::new(panel.x + panel.w * 0.5, yy, panel.w * 0.5 - 20.0, 46.0), &right, 14.0, false, WHITE, Align::Right);
                yy += 50.0;
            }
            yy - start
        });
        if let Some(log) = &self.history.log {
            c.text(R::new(page.x + 18.0, bottom + 8.0, page.w - 36.0, 20.0), &format!("Every deleted file is listed in {log}"), 12.0, false, DIM, Align::Left);
        }
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
                st.junk = Junk { phase: Phase::Finding, ..Junk::default() };
                st.junk.job = Some(jobs::start(hwnd, json!({"op": "scan"})));
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
            with(|st| {
                if !st.junk.selected.remove(&id) {
                    st.junk.selected.insert(id);
                }
            });
        }
        Action::JunkClean => {
            let Some(size) = with(|st| st.junk_selected_bytes()) else { return };
            if confirm(hwnd, &format!("Remove {} of junk? This cannot be undone.", bytes(Some(size)))) {
                with(|st| {
                    let plan = st.junk.plan["plan"].as_u64().unwrap_or(0);
                    let rules: Vec<String> = st.junk.selected.iter().cloned().collect();
                    st.junk.phase = Phase::Acting;
                    st.junk.job = Some(jobs::start(hwnd, json!({"op": "clean", "plan": plan, "rules": rules})));
                    st.update_timers();
                });
            }
        }
        Action::JunkReset => {
            with(|st| st.junk = Junk::default());
        }
        Action::PickFind(m) => {
            with(|st| {
                let req = if m == Module::Installers { json!({"op": "installers"}) } else { json!({"op": "projects"}) };
                let job = jobs::start(hwnd, req);
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
                let p = st.picks(m);
                if !p.selected.remove(&i) {
                    p.selected.insert(i);
                }
            });
        }
        Action::PickAct(m) => {
            let Some((n, size)) = with(|st| {
                let p = st.picks(m);
                let size: f64 = p.selected.iter().filter_map(|&i| items_of(&p.list).nth(i)).filter_map(|it| it["bytes"].as_f64()).sum();
                (p.selected.len(), size)
            }) else { return };
            let text = if m == Module::Installers {
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
                    p.phase = Phase::Acting;
                    p.job = Some(jobs::start(hwnd, json!({"op": "remove", "plan": plan, "items": items})));
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
