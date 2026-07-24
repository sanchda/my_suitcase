//! ratatui rendering. Pure formatting helpers are unit-tested; `render` is
//! exercised by the smoke test in Task 11.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Row, Table};
use crate::usage::Window;

pub fn human_tokens(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{}k", n / 1_000) }
    else { n.to_string() }
}

/// Width of the Instances panel's `model` column: wide enough for the longest
/// shortened model name (`sonnet-4-6`) so names aren't clipped mid-word.
const MODEL_COL_WIDTH: u16 = 11;

/// Drop the redundant `claude-` vendor prefix so the model column shows the
/// part that actually distinguishes one row from another. Anything that isn't a
/// prefixed model id (e.g. `<synthetic>`) is passed through untouched.
pub fn short_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

pub fn fmt_cost(c: Option<f64>) -> String {
    match c { Some(v) => format!("${v:.2}"), None => "$—".to_string() }
}

fn window_label(w: Window) -> &'static str {
    match w { Window::Today => "today", Window::Week => "7d", Window::All => "all" }
}

pub fn render(f: &mut Frame, app: &crate::AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(6), Constraint::Min(6), Constraint::Length(1)])
        .split(f.area());

    let acct = app.header_account.as_deref().unwrap_or("(unknown account)");
    let header = Line::from(vec![
        Span::styled("claude-top", Style::new().bold()),
        Span::raw(format!("  {acct}   [{}]   q:quit  t:window", window_label(app.window))),
    ]);
    f.render_widget(header, chunks[0]);

    // Instances
    let mut rows: Vec<Row> = Vec::new();
    for i in &app.instances {
        let wt = i.worktree.as_ref().map(|w| format!(" (wt:{w})")).unwrap_or_default();
        rows.push(Row::new(vec![
            Cell::from(i.pid.to_string()),
            Cell::from(i.tmux.clone().unwrap_or_default()),
            Cell::from(i.dir.clone().unwrap_or_default()),
            Cell::from(format!("{}{}", i.branch.clone().unwrap_or_default(), wt)),
            Cell::from(i.model.as_deref().map(short_model).unwrap_or_default().to_string()),
            Cell::from(human_tokens(i.session_tokens)),
            Cell::from(fmt_cost(i.session_cost)),
        ]));
    }
    let widths = [Constraint::Length(7), Constraint::Length(8), Constraint::Min(16), Constraint::Min(16), Constraint::Length(MODEL_COL_WIDTH), Constraint::Length(8), Constraint::Length(9)];
    let instances = Table::new(rows, widths)
        .header(Row::new(vec!["PID", "tmux", "dir", "branch/worktree", "model", "tok", "est $"]).style(Style::new().bold()))
        .block(Block::default().borders(Borders::ALL).title("Instances"));
    f.render_widget(instances, chunks[1]);

    // Usage
    let mut urows: Vec<Row> = Vec::new();
    for m in &app.by_model {
        urows.push(Row::new(vec![
            Cell::from(m.model.clone()),
            Cell::from(format!("{} / {} / {}", human_tokens(m.toks.input), human_tokens(m.toks.output), human_tokens(m.toks.cache_read + m.toks.cache_write))),
            Cell::from(fmt_cost(m.cost_usd)),
        ]));
    }
    urows.push(Row::new(vec![Cell::from("— by instance —")]));
    for inst in &app.by_instance {
        urows.push(Row::new(vec![
            Cell::from(inst.session_id.chars().take(8).collect::<String>()),
            Cell::from(format!("{}  {}", inst.model.clone().unwrap_or_default(), human_tokens(inst.tokens))),
            Cell::from(fmt_cost(inst.cost_usd)),
        ]));
    }
    let uwidths = [Constraint::Min(14), Constraint::Min(24), Constraint::Length(10)];
    let usage = Table::new(urows, uwidths)
        .header(Row::new(vec!["model/session", "tokens (in/out/cache)", "est $"]).style(Style::new().bold()))
        .block(Block::default().borders(Borders::ALL).title(format!("Usage — current account, {}", window_label(app.window))));
    f.render_widget(usage, chunks[2]);

    let footer = Line::from(Span::styled(app.footer.clone(), Style::new().dim()));
    f.render_widget(footer, chunks[3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_tokens() {
        assert_eq!(human_tokens(950), "950");
        assert_eq!(human_tokens(1_500), "1k");
        assert_eq!(human_tokens(1_100_000), "1.1M");
    }

    #[test]
    fn shortens_model_names_by_dropping_the_vendor_prefix() {
        assert_eq!(short_model("claude-opus-5"), "opus-5");
        assert_eq!(short_model("claude-sonnet-4-6"), "sonnet-4-6");
        assert_eq!(short_model("claude-haiku-4-5"), "haiku-4-5");
    }

    #[test]
    fn leaves_unprefixed_model_names_alone() {
        // Not every `model` field is a real model id.
        assert_eq!(short_model("<synthetic>"), "<synthetic>");
        assert_eq!(short_model(""), "");
    }

    #[test]
    fn short_model_fits_the_column() {
        for m in ["claude-opus-5", "claude-sonnet-4-6", "claude-haiku-4-5", "claude-fable-5"] {
            assert!(short_model(m).len() <= MODEL_COL_WIDTH as usize, "{m} overflows");
        }
    }

    #[test]
    fn formats_cost() {
        assert_eq!(fmt_cost(Some(4.1)), "$4.10");
        assert_eq!(fmt_cost(None), "$—");
    }
}
