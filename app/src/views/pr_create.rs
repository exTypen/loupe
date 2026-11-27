//! The New Pull Request sheet (PullCreate.swift) and, in `offer`, whether to offer one: the Changes banner, the Pull
//! Requests toolbar button and the sidebar menu item all use `offer` / `can_create` / `show_menu_item` / `banner`.
//!
//! Entry point: `install` subscribes to `AppEvent::CreatePull` and presents `PullCreateSheet`. On success the sheet
//! closes and emits `AppEvent::OpenPull`, which selects the worktree, switches to Pull Requests and opens the PR.
//! The pure parts (defaults, template, AI prompt, push-then-create) live in `loupe_forge::pull_create`.
mod offer;

pub use offer::{Offer, banner, can_create, invalidate, offer, show_menu_item};

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px,
    component::{
        IndexPath, Sizable as _,
        checkbox::Checkbox,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        select::{Select, SelectEvent, SelectState},
        spinner::Spinner,
        tooltip::Tooltip,
    },
};
use loupe_forge::{Client, Error, PullRequest, RemoteRepo, ai, client::new_pull, git as fgit, pull_create::{self, BaseState, PullCreate}, settings};

use crate::state::{AppEvent, AppModel, WorktreeModel};
use crate::{theme, ui};

pub fn install(app: &Entity<AppModel>, window: &mut Window, cx: &mut App) -> Subscription {
    flows_open(app, cx);
    window.subscribe(app, cx, |app, e: &AppEvent, window, cx| {
        let AppEvent::CreatePull { worktree } = e else { return };
        let Some(model) = app.read(cx).model(worktree) else { return };
        let v = cx.new(|cx| PullCreateSheet::new(app.clone(), model, window, cx));
        let f = v.read(cx).focus.clone();
        app.update(cx, |a, cx| a.present(v.into(), cx));
        window.focus(&f, cx);
    })
}

/// Flows runs only (LOUPE_STRESS): `LOUPE_FLOWS_CREATE_PULL=<worktree>` opens the sheet once that worktree is
/// listed, so the sheet can be compared with the Swift one before the sidebar menu and the Changes banner (other
/// owners' files) call into this module.
fn flows_open(app: &Entity<AppModel>, cx: &mut App) {
    let Some(path) = std::env::var("LOUPE_FLOWS_CREATE_PULL").ok().filter(|_| app.read(cx).scenario.is_some()) else { return };
    let mut found = false;
    cx.observe(app, move |app, cx| {
        let Some(m) = app.read(cx).model(&path).filter(|_| !found) else { return };
        found = true;
        let (app, path, mut done) = (app.clone(), path.clone(), false);
        // The remote and status arrive after the model exists, and they notify only the model.
        cx.observe(&m, move |m, cx| {
            if done || !m.read(cx).remote_detected || m.read(cx).state.head_oid.is_empty() { return }
            done = true;
            let (app, path) = (app.clone(), path.clone());
            cx.defer(move |cx| app.update(cx, |a, cx| { a.select(Some(path.clone()), cx); a.emit(AppEvent::CreatePull { worktree: path }, cx) }));
        }).detach();
    }).detach();
}

/// The base picker's list.
type Bases = SelectState<Vec<String>>;

pub struct PullCreateSheet {
    app: Entity<AppModel>,
    model: Entity<WorktreeModel>,
    path: String,
    remote_name: String,
    remote: Option<RemoteRepo>,
    client: Option<Client>,
    pc: PullCreate,
    loading: bool,
    /// "Pushing", "Creating", "Writing", "Fetching".
    working: Option<&'static str>,
    error: Option<String>,
    /// The chosen base has no remote-tracking ref: Swift silently showed "No commits…" (§14, PullCreate.swift:207).
    unfetched: bool,
    harness: Option<ai::Selected>,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    bases: Option<Entity<Bases>>,
    focus: FocusHandle,
    /// Dropped with the sheet: Cancel stops waiting, so a push or create that finishes later no longer navigates
    /// (§14, PullCreate.swift:363).
    task: Option<Task<()>>,
    base_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl PullCreateSheet {
    pub fn new(app: Entity<AppModel>, model: Entity<WorktreeModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (path, remote_name, remote, branch) = {
            let m = model.read(cx);
            (m.path_str(), m.state.remote_name.clone(), m.remote.clone(), m.state.branch.clone().unwrap_or_default())
        };
        let client = remote.as_ref().and_then(|r| app.read(cx).client_for(r, &path));
        offer::undismiss(&path, &branch);
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder("Description"));
        let subs = vec![
            cx.subscribe(&title, |v, i, e: &InputEvent, cx| match e {
                InputEvent::Change => { v.pc.title = i.read(cx).value().to_string(); cx.notify() }
                InputEvent::PressEnter { .. } => v.create(cx),
                _ => {}
            }),
            cx.subscribe(&body, |v, i, e: &InputEvent, cx| if let InputEvent::Change = e { v.pc.body = i.read(cx).value().to_string() }),
        ];
        let mut v = Self {
            app, model, path, remote_name, remote, client, pc: { let mut pc = PullCreate::default(); pc.branch = branch; pc }, loading: true,
            working: None, error: None, unfetched: false, harness: None, title, body, bases: None, focus: cx.focus_handle(),
            task: None, base_task: None, _subs: subs,
        };
        v.load(window, cx);
        v
    }

    /// PC-07: bases, the default one, the template, an open PR, then the first base's commits. Once.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(client), Some(remote)) = (self.client.clone(), self.remote.clone()) else {
            self.error = Some("No account for this repository's remote.".into());
            self.loading = false;
            return;
        };
        let (dir, rn, branch) = (self.path.clone(), self.remote_name.clone(), self.pc.branch.clone());
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move {
                let pc = PullCreate::load(&client, &remote, &dir, &rn, &branch)?;
                let st = base_state(&dir, &rn, &pc.base, &branch);
                // The AI tools on PATH, looked up once per run (Settings' "Look Again" does it again).
                let paths = ai::installed().unwrap_or_else(ai::scan);
                let harness = ai::selected(&settings::shared(), &paths);
                Ok::<_, Error>((pc, st, harness))
            }).await;
            this.update_in(cx, |v, window, cx| {
                v.loading = false;
                match r {
                    Ok((pc, (st, unfetched), harness)) => {
                        v.pc = pc;
                        v.harness = harness;
                        v.apply_base(st, unfetched, window, cx);
                        let sel = v.pc.bases.iter().position(|b| *b == v.pc.base).map(IndexPath::new);
                        let bases = cx.new(|cx| Bases::new(v.pc.bases.clone(), sel, window, cx));
                        v._subs.push(cx.subscribe_in(&bases, window, |v, _, e: &SelectEvent<Vec<String>>, window, cx| {
                            let SelectEvent::Confirm(Some(b)) = e else { return };
                            if *b != v.pc.base { v.pc.base = b.clone(); v.base_changed(window, cx) }
                        }));
                        v.bases = Some(bases);
                        v.title.update(cx, |i, cx| i.focus(window, cx));
                    }
                    Err(e) => v.error = e.user_message(),
                }
                cx.notify();
            }).ok();
        }));
    }

    /// PC-08: commits, title, body and "pushed first" follow the base until edited; a stale answer is dropped.
    fn base_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (dir, rn, base, branch) = (self.path.clone(), self.remote_name.clone(), self.pc.base.clone(), self.pc.branch.clone());
        self.base_task = Some(cx.spawn_in(window, async move |this, cx| {
            let (st, unfetched) = cx.background_executor().spawn(async move { base_state(&dir, &rn, &base, &branch) }).await;
            this.update_in(cx, |v, window, cx| { v.apply_base(st, unfetched, window, cx); cx.notify() }).ok();
        }));
        cx.notify();
    }

    fn apply_base(&mut self, st: BaseState, unfetched: bool, window: &mut Window, cx: &mut Context<Self>) {
        if st.base != self.pc.base { return }
        self.unfetched = unfetched;
        self.pc.apply_base(st);
        self.sync_inputs(window, cx);
    }

    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (t, b) = (self.pc.title.clone(), self.pc.body.clone());
        if self.title.read(cx).value().as_ref() != t { self.title.update(cx, |i, cx| i.set_value(t, window, cx)) }
        if self.body.read(cx).value().as_ref() != b { self.body.update(cx, |i, cx| i.set_value(b, window, cx)) }
    }

    /// The fix for an unfetched default branch: fetch, then look at the base again.
    fn fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.model.update(cx, |m, cx| m.fetch(cx));
        self.working = Some("Fetching");
        cx.notify();
        self.base_task = Some(cx.spawn_in(window, async move |this, cx| {
            t.await;
            this.update_in(cx, |v, window, cx| { v.working = None; v.base_changed(window, cx) }).ok();
        }));
    }

    /// PC-11. The reply's body replaces the description only while the user hasn't edited it: Swift always
    /// overwrote an edited one (§14, PullCreate.swift:243).
    fn write_with_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.harness.clone() else { return };
        if self.pc.commits.is_empty() || self.working.is_some() { return }
        self.working = Some("Writing");
        self.error = None;
        cx.notify();
        let (pc, dir, rn) = (self.pc.clone(), self.path.clone(), self.remote_name.clone());
        let extra = ai::extra_instructions(&settings::shared());
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { pc.write_with_ai(&dir, &rn, &extra, &h, None) }).await;
            this.update_in(cx, |v, window, cx| {
                v.working = None;
                match r {
                    Ok((t, b)) => {
                        if !t.is_empty() { v.pc.title = t }
                        let auto = pull_create::body(&v.pc.commits, v.pc.template.as_deref());
                        if v.pc.body.trim().is_empty() || v.pc.body == auto { v.pc.body = b }
                        v.sync_inputs(window, cx);
                    }
                    Err(e) => if let Some(m) = e.user_message() { v.error = Some(m) },
                }
                cx.notify();
            }).ok();
        }));
    }

    fn can_create(&self) -> bool { !self.loading && self.working.is_none() && self.client.is_some() && self.pc.can_create() }

    /// PC-15..19: push when the remote doesn't have HEAD yet (the space's credentials), then create and show it.
    fn create(&mut self, cx: &mut Context<Self>) {
        if !self.can_create() { return }
        let (Some(client), Some(remote)) = (self.client.clone(), self.remote.clone()) else { return };
        self.error = None;
        self.pc.existing = None;
        let push = self.pc.needs_push;
        self.working = Some(if push { "Pushing" } else { "Creating" });
        cx.notify();
        let env = self.app.read(cx).git_auth(Some(&remote), &self.path);
        let (dir, rn, model) = (self.path.clone(), self.remote_name.clone(), self.model.clone());
        let p = new_pull(&self.pc.branch, &self.pc.base, &self.pc.title, &self.pc.body, self.pc.draft, client.github);
        let branch = self.pc.branch.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            if push {
                let r = cx.background_executor().spawn(async move { fgit::run(&["push", "--set-upstream", &rn, "HEAD"], &dir, &env, None) }).await;
                let ok = this.update(cx, |v, cx| match r {
                    Ok(_) => {
                        v.pc.needs_push = false;
                        v.working = Some("Creating");
                        // Remote refs moved: the offer and History key on refs_version (Swift didn't bump it, §14
                        // PullCreate.swift:255).
                        model.update(cx, |m, cx| { m.state.bump_refs(); m.refresh(cx) });
                        cx.notify();
                        true
                    }
                    Err(e) => { v.working = None; v.error = e.user_message(); cx.notify(); false }
                }).unwrap_or(false);
                if !ok { return }
            }
            let r = cx.background_executor().spawn(async move {
                match client.create_pull(&remote, &p) {
                    Err(Error::PullExists) => Err((Error::PullExists, client.open_pull(&remote, &branch).ok().flatten())),
                    r => r.map_err(|e| (e, None)),
                }
            }).await;
            this.update(cx, |v, cx| {
                v.working = None;
                match r {
                    Ok(pr) => v.show(&pr, cx),
                    Err((e, existing)) => { v.error = e.user_message(); v.pc.existing = existing; cx.notify() }
                }
            }).ok();
        }));
    }

    /// PC-17: close, then the Pull Requests tab opens the PR (selecting this worktree first). No toast.
    fn show(&mut self, pr: &PullRequest, cx: &mut Context<Self>) {
        let (path, n) = (self.path.clone(), pr.number as u64);
        self.close(cx);
        self.app.update(cx, |a, cx| a.emit(AppEvent::OpenPull { worktree: path, number: n }, cx));
    }

    /// PC-20 and every way out: the banner checks again (PC-04).
    fn close(&mut self, cx: &mut Context<Self>) {
        offer::invalidate(&self.path, cx);
        self.model.update(cx, |_, cx| cx.notify());
        self.app.update(cx, |a, cx| a.dismiss(cx));
    }
}

/// `PullCreate::branch_state`, plus whether `<remote>/<base>` exists at all (nothing to compare with when not).
fn base_state(dir: &str, remote_name: &str, base: &str, branch: &str) -> (BaseState, bool) {
    let st = PullCreate::branch_state(dir, remote_name, base, branch, None);
    let unfetched = !base.is_empty() && st.total == 0
        && fgit::run(&["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{remote_name}/{base}")], dir, &[], None).is_err();
    (st, unfetched)
}

impl Focusable for PullCreateSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus.clone() }
}

fn row(label: &str, content: impl IntoElement) -> gpui_kit::Div {
    div().flex().items_center().gap_3().min_h(px(30.))
        .child(div().w(px(84.)).flex_none().text_color(theme::text_muted()).child(label.to_string()))
        .child(div().flex_1().min_w_0().flex().items_center().gap_2().child(content))
}

fn field() -> gpui_kit::Div {
    div().flex_1().min_w_0().px_2().py_1().rounded(theme::RADIUS_ROW).bg(theme::raised()).border_1().border_color(theme::border())
}

impl Render for PullCreateSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pc = &self.pc;
        let github = self.client.as_ref().is_some_and(|c| c.github);
        let max_h = window.viewport_size().height - px(64.);
        let branch: SharedString = pc.branch.clone().into();
        let commits_title = if pc.total == 1 { "1 commit".to_string() } else { format!("{} commits", pc.total) };
        let busy = self.working.is_some_and(|w| w != "Writing");
        let can = self.can_create();

        let ai = self.harness.as_ref().map(|h| h.harness.name).map(|name| if self.working == Some("Writing") {
            div().size(px(28.)).flex().items_center().justify_center().child(Spinner::new().small()).into_any_element()
        } else {
            let off = pc.commits.is_empty() || self.working.is_some();
            ui::icon_button("pc-ai", "sparkles")
                .map(|b| if off { b.opacity(0.4).cursor_default() } else { b.text_color(theme::blue()) })
                .tooltip(move |w, cx| Tooltip::new(format!("Write the title and description with {name}, from the commits and their diff")).build(w, cx))
                .on_click(cx.listener(move |v, _, w, cx| if !off { v.write_with_ai(w, cx) }))
                .into_any_element()
        });

        let form = div().flex().flex_col().gap_2().p_4()
            .child(row("Branch", div().flex().items_center().gap_2().min_w_0()
                .child(div().id("pc-branch").min_w_0().truncate().font_family(theme::CODE_FONT).text_size(theme::CODE_SIZE).text_color(theme::text())
                    .child(branch.clone()).tooltip(move |w, cx| Tooltip::new(branch.clone()).build(w, cx)))
                .when(pc.needs_push, |d| d.child(div().flex_none().text_size(px(11.)).text_color(theme::text_muted()).child("pushed first")))))
            .child(row("Into", match &self.bases {
                Some(b) => div().w(px(260.)).child(Select::new(b).small().menu_max_h(px(320.)).disabled(self.working.is_some())).into_any_element(),
                None => div().text_color(theme::text_faint()).child(pc.base.clone()).into_any_element(),
            }))
            .child(row("Title", div().flex().flex_1().items_center().gap_2()
                .child(field().child(Input::new(&self.title).appearance(false).small()))
                .children(ai)))
            .child(div().h(px(160.)).px_2().py_1().rounded(theme::RADIUS_ROW).bg(theme::raised()).border_1().border_color(theme::border())
                .text_color(theme::text()).child(Textarea::new(&self.body).appearance(false).h_full()))
            .child(div().pt_1().child(Checkbox::new("pc-draft").label("Draft").checked(pc.draft)
                .tooltip(if github { "Open it as a draft pull request" } else { "Open it as a draft: the title starts with “WIP: ”" })
                .on_click(cx.listener(|v, on: &bool, _, cx| { v.pc.draft = *on; cx.notify() }))));

        let commits = div().flex().flex_col().px_4().pb_3().gap_0p5()
            .child(ui::section_header(commits_title, None).px_0())
            .when(pc.commits.is_empty() && !self.loading, |d| d.child(div().text_color(theme::text_muted()).child(
                format!("No commits on {} that aren't on {}.", pc.branch, pc.base))))
            .when(self.unfetched, |d| d.child(div().flex().items_center().gap_2().text_size(px(12.)).text_color(theme::accent())
                .child(format!("{}/{} hasn't been fetched.", self.remote_name, pc.base))
                .child(ui::link_button("pc-fetch", "Fetch").on_click(cx.listener(|v, _, w, cx| v.fetch(w, cx))))))
            .children(pc.commits.iter().enumerate().map(|(i, c)| {
                let tip: SharedString = if c.body.is_empty() { c.subject.clone() } else { format!("{}\n\n{}", c.subject, c.body) }.into();
                div().id(("pc-commit", i)).flex().items_center().gap_2().h(px(22.))
                    .tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx))
                    .child(div().flex_none().font_family(theme::CODE_FONT).text_size(px(11.)).text_color(theme::text_muted()).child(c.sha.chars().take(7).collect::<String>()))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::text()).child(c.subject.clone()))
            }))
            .when(pc.total > pc.commits.len(), |d| d.child(div().text_color(theme::text_muted()).child(format!("and {} more", pc.total - pc.commits.len()))));

        let problems = (self.error.is_some() || pc.existing.is_some()).then(|| div().flex().flex_col().gap_2().mx_4().mb_3().p_3()
            .rounded(theme::RADIUS_ROW).border_1().border_color(theme::border()).bg(theme::card_header())
            .when_some(self.error.clone(), |d, e| d.child(div().text_size(px(12.5)).text_color(theme::red()).child(e)))
            .when_some(pc.existing.clone(), |d, pr| d.child(div().flex().items_center().gap_2()
                .child(div().flex_1().min_w_0().truncate().text_color(theme::text()).child(format!("#{} {}", pr.number, pr.title)))
                .child(ui::pill("pc-open-existing", "Open Pull Request").on_click(cx.listener(move |v, _, _, cx| v.show(&pr, cx)))))));

        let footer = div().flex().items_center().gap_2().px_4().h(px(48.)).flex_none().border_t_1().border_color(theme::border())
            .child(div().flex_1())
            .child(ui::pill("pc-cancel", "Cancel").on_click(cx.listener(|v, _, _, cx| v.close(cx))))
            .when(busy, |d| d.child(Spinner::new().small()))
            .child(ui::pill_primary("pc-create", self.pc.create_label(), theme::blue())
                .when(!can, |b| b.opacity(0.45).cursor_default())
                .on_click(cx.listener(|v, _, _, cx| v.create(cx))));

        div().absolute().inset_0().flex().items_center().justify_center().bg(theme::backdrop())
            .track_focus(&self.focus)
            // A click on a button or a label drops focus to the window, and Esc (Cancel, PC-20) went nowhere: take it
            // back. Inputs focus themselves after this.
            .on_mouse_down(gpui_kit::MouseButton::Left, cx.listener(|v, _, w, cx| if !v.focus.contains_focused(w, cx) { w.focus(&v.focus, cx) }))
            // Esc cancels from anywhere in the sheet, as Swift's cancelAction did (PC-20): the fields, the checkbox
            // and the buttons swallowed it before it bubbled here. An open base menu (focus in its list, below the
            // trigger) still closes first.
            .capture_key_down(cx.listener(|v, e: &gpui_kit::KeyDownEvent, w, cx| {
                if e.keystroke.key != "escape" { return }
                let menu_open = v.bases.as_ref().is_some_and(|b| { let f = b.focus_handle(cx); f.contains_focused(w, cx) && !f.is_focused(w) });
                if !menu_open { v.close(cx); cx.stop_propagation() }
            }))
            .child(ui::card().w(px(560.)).min_h(px(520.)).max_h(max_h).bg(theme::sidebar()).text_size(theme::UI_SIZE)
                .child(ui::card_header(div().font_weight(FontWeight::SEMIBOLD).child("New Pull Request"), div()))
                .child(if self.loading {
                    div().flex_1().flex().items_center().justify_center().child(Spinner::new()).into_any_element()
                } else {
                    div().id("pc-body").flex_1().min_h_0().overflow_y_scroll().flex().flex_col()
                        .child(form).child(commits).children(problems).into_any_element()
                })
                .child(footer))
    }
}
