use super::{events::capture_key_press, state};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    tty::IsTty,
    ExecutableCommand,
};
use human_format::{Formatter, Scales};
use num_format::{Locale, ToFormattedString};
use std::io::{stdout, Write};
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph, Row, Table, Wrap},
    Frame, Terminal,
};

/// Format metrics, with thousands separation
trait ThousandsFormatter {
    fn format_thousands(&self) -> String;
}

impl ThousandsFormatter for i64 {
    fn format_thousands(&self) -> String {
        match self {
            0 => "--".into(),
            _ => self.to_formatted_string(&Locale::en),
        }
    }
}

/// Format metrics, using the 'humanized' format, abbreviating with suffixes
trait HumanFormatter {
    fn format_human(&self) -> String;
    fn format_human_bytes(&self) -> String;
}

impl HumanFormatter for i64 {
    /// Format an i64 as a string, returning `--` if zero, the value as a string if < 1000, or
    /// the value abbreviated with 'k' if at least 1,000
    fn format_human(&self) -> String {
        match self {
            0 => "--".into(),
            1..=999 => self.to_string(),
            _ => Formatter::new().with_decimals(2).format(*self as f64),
        }
    }

    /// Format an i64 as a string that represents a binary size
    fn format_human_bytes(&self) -> String {
        match self {
            0 => "--".into(),
            _ => Formatter::new()
                .with_scales(Scales::SI())
                .format(*self as f64),
        }
    }
}

struct Widgets<'a> {
    constraints: Vec<Constraint>,
    url_string: &'a str,
    human_metrics: bool,
}

impl<'a> Widgets<'a> {
    /// Creates a new Widgets, containing constraints to re-use across renders.
    pub fn new(url_string: &'a str, human_metrics: bool) -> Self {
        let constraints = vec![
            Constraint::Length(3),
            Constraint::Max(90),
            Constraint::Length(3),
        ];

        Self {
            constraints,
            url_string,
            human_metrics,
        }
    }

    /// Renders a title showing 'Vector', and the URL the dashboard is currently connected to.
    fn title<B: Backend>(&'a self, f: &mut Frame<B>, area: Rect) {
        let text = vec![Spans::from(self.url_string)];

        let block = Block::default().borders(Borders::ALL).title(Span::styled(
            "Vector",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
        let w = Paragraph::new(text).block(block).wrap(Wrap { trim: true });

        f.render_widget(w, area);
    }

    /// Renders a components table, showing sources, transforms and sinks in tabular form, with
    /// statistics pulled from `ComponentsState`,
    fn components_table<B: Backend>(&self, f: &mut Frame<B>, state: &state::State, area: Rect) {
        let items = state.iter().map(|(_, r)| {
            let mut data = vec![r.name.clone(), r.kind.clone(), r.component_type.clone()];

            // Build metric stats
            let formatted_metrics = if self.human_metrics {
                [
                    r.events_processed_total.format_human(),
                    r.bytes_processed_total.format_human_bytes(),
                    r.errors.format_human(),
                ]
            } else {
                [
                    r.events_processed_total.format_thousands(),
                    r.bytes_processed_total.format_thousands(),
                    r.errors.format_thousands(),
                ]
            };

            data.extend_from_slice(&formatted_metrics);
            Row::StyledData(data.into_iter(), Style::default().fg(Color::White))
        });

        let w = Table::new(state::COMPONENT_HEADERS.iter(), items)
            .block(Block::default().borders(Borders::ALL).title("Components"))
            .header_gap(1)
            .column_spacing(2)
            .widths(&[
                Constraint::Percentage(15),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
            ]);

        f.render_widget(w, area);
    }

    /// Renders a box showing instructions on how to exit from `vector top`.
    fn quit_box<B: Backend>(&self, f: &mut Frame<B>, area: Rect) {
        let text = vec![Spans::from("To quit, press ESC or 'q'")];

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray));
        let w = Paragraph::new(text)
            .block(block)
            .style(Style::default().fg(Color::Gray))
            .alignment(Alignment::Left);

        f.render_widget(w, area);
    }

    /// Draw a single frame. Creates a layout and renders widgets into it.
    fn draw<B: Backend>(&self, f: &mut Frame<B>, state: state::State) {
        let rects = Layout::default()
            .constraints(self.constraints.as_ref())
            .split(f.size());

        self.title(f, rects[0]);
        self.components_table(f, &state, rects[1]);
        self.quit_box(f, rects[2]);
    }
}

/// Determine if the terminal is a TTY
pub fn is_tty() -> bool {
    stdout().is_tty()
}

/// Initialize the dashboard. A new terminal drawing session will be created, targeting
/// stdout. We're using 'direct' drawing mode to control the full output of the dashboard,
/// as well as entering an 'alternate screen' to overlay the console. This ensures that when
/// the dashboard is exited, the user's previous terminal session can commence, unaffected.
pub async fn init_dashboard(
    url: &str,
    human_metrics: bool,
    mut state_rx: state::StateRx,
) -> Result<(), Box<dyn std::error::Error>> {
    // Capture key presses, to determine when to quit
    let (mut key_press_rx, key_press_kill_tx) = capture_key_press();

    // Write to stdout, and enter an alternate screen, to avoid overwriting existing
    // terminal output
    let mut stdout = stdout();

    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    // Drop into 'raw' mode, to enable direct drawing to the terminal
    enable_raw_mode()?;

    // Build terminal. We're using crossterm for *nix + Windows support
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Clear the screen, readying it for output
    terminal.clear()?;

    let widgets = Widgets::new(url, human_metrics);

    loop {
        tokio::select! {
            Some(state) = state_rx.recv() => {
                terminal.draw(|f| widgets.draw(f, state))?;
            },
            k = key_press_rx.recv() => {
                if let KeyCode::Esc | KeyCode::Char('q') = k.unwrap() {
                    let _ = key_press_kill_tx.send(());
                    break
                }
            }
        }
    }

    // Clean-up terminal
    terminal.backend_mut().execute(DisableMouseCapture)?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;

    disable_raw_mode()?;

    Ok(())
}
