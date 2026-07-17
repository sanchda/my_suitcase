mod discover;
mod git;
mod pricing;
mod runtime;
mod tmux;
mod ui;
mod usage;

pub struct InstanceRow {
    pub pid: u32,
    // Per-instance account is collected but the UI folds account identity into
    // the header (there's one account per running claude-top process); kept
    // here for future per-row display if the UI grows a column for it.
    #[allow(dead_code)]
    pub account: Option<String>,
    pub tmux: Option<String>,
    pub dir: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub model: Option<String>,
    pub session_tokens: u64,
    pub session_cost: Option<f64>,
    pub session_id: Option<String>,
}

pub struct AppState {
    pub header_account: Option<String>,
    pub window: crate::usage::Window,
    pub instances: Vec<InstanceRow>,
    pub by_model: Vec<crate::usage::ModelUsage>,
    pub by_instance: Vec<crate::usage::InstanceUsage>,
    pub footer: String,
}

use std::collections::{HashMap, HashSet};
use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::execute;
use ratatui::prelude::*;

use crate::usage::{InstanceUsage, Window};

fn cycle(w: Window) -> Window {
    match w { Window::Today => Window::Week, Window::Week => Window::All, Window::All => Window::Today }
}

fn build_state(rt: &runtime::Runtime, window: Window) -> AppState {
    let (mut instances, running, header_account, footer) = rt.collect_instances();
    let running_set: HashSet<String> = running.iter().cloned().collect();
    let snap = rt.usage_snapshot(window, &running_set);

    // The Instances panel's model/tok columns are populated from the usage
    // snapshot's by_instance list (already scoped to running sessions),
    // keyed by session id.
    let by_sid: HashMap<&str, &InstanceUsage> = snap.by_instance.iter().map(|u| (u.session_id.as_str(), u)).collect();
    for row in instances.iter_mut() {
        if let Some(sid) = row.session_id.as_deref() {
            if let Some(entry) = by_sid.get(sid) {
                row.model = entry.model.clone();
                row.session_tokens = entry.tokens;
                row.session_cost = entry.cost_usd;
            }
        }
    }

    AppState {
        header_account,
        window,
        instances,
        by_model: snap.by_model,
        by_instance: snap.by_instance,
        footer,
    }
}

fn main() -> Result<()> {
    // Non-TUI fast paths.
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => { println!("claude-top {}", env!("CARGO_PKG_VERSION")); return Ok(()); }
        Some("--help") | Some("-h") => { println!("claude-top — live view of local Claude Code instances (q: quit, t: window)"); return Ok(()); }
        _ => {}
    }

    enable_raw_mode()?;
    // Guard must be created immediately after raw mode is enabled so that
    // any failure in the following setup steps (EnterAlternateScreen,
    // Terminal::new) still triggers a best-effort restore on drop. It also
    // covers errors/panics from `run()` itself, since Drop runs on unwind.
    let _guard = TerminalGuard;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    run(&mut terminal)
}

/// RAII guard that unconditionally (best-effort) restores the terminal to
/// its normal state on drop, regardless of whether that happens via a
/// normal return, an early `?` propagation, or a panic unwind. Individual
/// restore steps ignore their own errors so one failing step never
/// prevents the rest from running.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        let _ = execute!(stdout(), crossterm::cursor::Show);
    }
}

fn run<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut rt = runtime::Runtime::new();
    let config_dir = discover::default_config_dir();
    let mut window = Window::Today;

    let inst_every = Duration::from_secs(2);
    let usage_every = Duration::from_secs(5);
    let mut last_inst = Instant::now().checked_sub(inst_every).unwrap_or_else(Instant::now);
    let mut last_usage = Instant::now().checked_sub(usage_every).unwrap_or_else(Instant::now);

    let mut state = {
        rt.refresh_usage(&config_dir);
        build_state(&rt, window)
    };

    loop {
        terminal.draw(|f| ui::render(f, &state))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('t') => { window = cycle(window); state = build_state(&rt, window); }
                    _ => {}
                }
            }
        }

        let now = Instant::now();
        let mut dirty = false;
        if now.duration_since(last_usage) >= usage_every { rt.refresh_usage(&config_dir); last_usage = now; dirty = true; }
        if now.duration_since(last_inst) >= inst_every { last_inst = now; dirty = true; }
        if dirty { state = build_state(&rt, window); }
    }
    Ok(())
}
