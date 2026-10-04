use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use super::debug_pane::DebugLog;
use super::model_panel::{self, ModelPanel, ModelSettings, Outcome};
use crate::agent::{ModelChoice, Trace, Verdict};
use crate::watcher::Change;

const TICK: Duration = Duration::from_millis(100);
/// From this width on, the debug pane sits beside the changes, not below.
const SIDE_BY_SIDE_WIDTH: u16 = 140;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub enum Update {
    Changes(Vec<Change>),
    Reviewing,
    Verdict(Result<Verdict>),
    Trace(Trace),
    WatchErrors(Vec<String>),
    NotifyFailed(String),
}

pub struct Screen {
    root: PathBuf,
    status: Vec<String>,
    log: Vec<Line<'static>>,
    lines_from_bottom: usize,
    debug: DebugLog,
    debug_lines_from_bottom: usize,
    debug_shown: bool,
    debug_focused: bool,
    reviewing_since: Option<Instant>,
    settings: Option<ModelSettings>,
    panel: Option<ModelPanel>,
    quit: bool,
}

impl Screen {
    pub fn new(root: &Path, status: Vec<String>, settings: Option<ModelSettings>) -> Self {
        Self {
            root: root.to_path_buf(),
            status,
            log: Vec::new(),
            lines_from_bottom: 0,
            debug: DebugLog::default(),
            debug_lines_from_bottom: 0,
            debug_shown: false,
            debug_focused: false,
            reviewing_since: None,
            settings,
            panel: None,
            quit: false,
        }
    }

    pub fn run(mut self, updates: Receiver<Update>) -> Result<()> {
        ratatui::run(|terminal| self.event_loop(terminal, updates))
    }

    fn event_loop(
        &mut self,
        terminal: &mut DefaultTerminal,
        updates: Receiver<Update>,
    ) -> Result<()> {
        while !self.quit {
            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(TICK)?
                && let Event::Key(key) = event::read()?
                && key.is_press()
            {
                self.handle_key(key);
            }
            loop {
                match updates.try_recv() {
                    Ok(update) => self.apply(update),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.reviewing_since = None;
                        self.log.push(Line::from("Stopped watching.".red()));
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if let Some(panel) = &mut self.panel {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.quit = true;
                return;
            }
            match panel.handle_key(key) {
                Outcome::Open => {}
                Outcome::Closed => self.panel = None,
                Outcome::Chosen(choice) => {
                    self.panel = None;
                    self.use_model(choice);
                }
            }
            return;
        }
        match key.code {
            KeyCode::Char('m') => {
                self.panel = self.settings.as_ref().map(ModelPanel::open);
            }
            KeyCode::Char('d') => {
                self.debug_shown = !self.debug_shown;
                self.debug_focused = self.debug_shown;
            }
            KeyCode::Tab | KeyCode::BackTab if self.debug_shown => {
                self.debug_focused = !self.debug_focused;
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_down(1),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::End | KeyCode::Char('G') => *self.focused_scroll() = 0,
            _ => {}
        }
    }

    fn use_model(&mut self, choice: ModelChoice) {
        let Some(settings) = &self.settings else {
            return;
        };
        let described = model_panel::describe(&choice);
        if let Err(e) = settings.apply(choice) {
            self.log.push(Line::from(
                format!("Checking with {described} until you quit; could not save it: {e:#}")
                    .yellow(),
            ));
        }
    }

    fn focused_scroll(&mut self) -> &mut usize {
        if self.debug_focused {
            &mut self.debug_lines_from_bottom
        } else {
            &mut self.lines_from_bottom
        }
    }

    fn scroll_up(&mut self, lines: usize) {
        let scroll = self.focused_scroll();
        *scroll = scroll.saturating_add(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        let scroll = self.focused_scroll();
        *scroll = scroll.saturating_sub(lines);
    }

    fn apply(&mut self, update: Update) {
        if let Update::Verdict(_) = update {
            self.reviewing_since = None;
        }
        match update {
            Update::Changes(changes) => {
                for change in &changes {
                    self.log.push(change_line(&self.root, change));
                }
            }
            Update::Reviewing => {
                self.reviewing_since = Some(Instant::now());
                self.debug.review_started();
            }
            Update::Trace(trace) => self.debug.add(trace),
            Update::Verdict(Ok(verdict)) if verdict.contradicts => {
                let field = verdict
                    .dmp_field
                    .map(|field| format!(" ({field})"))
                    .unwrap_or_default();
                self.log.push(Line::from(vec![
                    Span::raw("  "),
                    format!("contradicts the DMP{field}:").red().bold(),
                    Span::raw(format!(" {}", verdict.explanation)),
                ]));
            }
            Update::Verdict(Ok(verdict)) => self.log.push(Line::from(vec![
                Span::raw("  "),
                "in line with the DMP:".green(),
                Span::raw(format!(" {}", verdict.explanation)),
            ])),
            Update::Verdict(Err(e)) => self
                .log
                .push(Line::from(format!("  review failed: {e:#}").red())),
            Update::WatchErrors(errors) => {
                for e in errors {
                    self.log.push(Line::from(format!("watch error: {e}").red()));
                }
            }
            Update::NotifyFailed(e) => self.log.push(Line::from(
                format!("  could not show a notification: {e}").yellow(),
            )),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let mut header_lines = vec![Line::from(vec![
            "Watching ".into(),
            self.root.display().to_string().bold(),
        ])];
        header_lines.extend(self.status.iter().cloned().map(Line::from));
        if let Some(settings) = &self.settings {
            let choice = settings.reviewer.choice();
            header_lines.push(Line::from(vec![
                "Using ".into(),
                model_panel::describe(&choice).bold(),
                format!(" from {}.", settings.url).into(),
            ]));
        }
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(header_lines.len() as u16 + 2),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        frame.render_widget(
            Paragraph::new(header_lines)
                .wrap(Wrap { trim: false })
                .block(
                    Block::bordered()
                        .border_type(BorderType::Rounded)
                        .title(" damap-helper ".bold()),
                ),
            header,
        );

        if self.debug_shown {
            let direction = if body.width >= SIDE_BY_SIDE_WIDTH {
                Direction::Horizontal
            } else {
                Direction::Vertical
            };
            let [log, debug] = Layout::new(
                direction,
                [Constraint::Percentage(40), Constraint::Percentage(60)],
            )
            .areas(body);
            self.draw_log(frame, log);
            self.draw_debug(frame, debug);
        } else {
            self.draw_log(frame, body);
        }

        let mut keys = vec![
            " q".bold(),
            " quit  ".into(),
            "↑↓ PgUp PgDn".bold(),
            " scroll  ".into(),
            "End".bold(),
            " latest  ".into(),
            "d".bold(),
            if self.debug_shown {
                " hide debug".into()
            } else {
                " debug".into()
            },
        ];
        if self.debug_shown {
            keys.extend(["  Tab".bold(), " switch pane".into()]);
        }
        if self.settings.is_some() {
            keys.extend(["  m".bold(), " model".into()]);
        }
        frame.render_widget(
            Line::from(keys).style(Style::new().fg(Color::DarkGray)),
            footer,
        );

        if let Some(panel) = &mut self.panel {
            panel.draw(frame, frame.area());
        }
    }

    fn draw_log(&mut self, frame: &mut Frame, area: Rect) {
        let block = pane(" Changes ", self.debug_shown && !self.debug_focused);
        if self.log.is_empty() {
            let waiting =
                Paragraph::new("No changes yet.".add_modifier(Modifier::DIM)).block(block);
            frame.render_widget(waiting, area);
            return;
        }
        let mut lines = self.log.clone();
        if let Some(since) = self.reviewing_since {
            let elapsed = since.elapsed();
            let frame = SPINNER[(elapsed.as_millis() / TICK.as_millis()) as usize % SPINNER.len()];
            lines.push(Line::from(vec![
                format!("  {frame} ").cyan(),
                format!("checking against the DMP… {}s", elapsed.as_secs())
                    .add_modifier(Modifier::DIM),
            ]));
        }
        draw_scrolled(frame, area, block, lines, &mut self.lines_from_bottom);
    }

    fn draw_debug(&mut self, frame: &mut Frame, area: Rect) {
        let block = pane(" Debug: what the AI sees and does ", self.debug_focused);
        if self.debug.is_empty() {
            let waiting = Paragraph::new(
                "Nothing sent to the AI yet; it starts with the next change."
                    .add_modifier(Modifier::DIM),
            )
            .wrap(Wrap { trim: false })
            .block(block);
            frame.render_widget(waiting, area);
            return;
        }
        let lines = self.debug.lines(block.inner(area).width).to_vec();
        draw_scrolled(frame, area, block, lines, &mut self.debug_lines_from_bottom);
    }
}

fn pane(title: &str, focused: bool) -> Block<'_> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title);
    if focused {
        block.border_style(Style::new().fg(Color::Cyan))
    } else {
        block
    }
}

/// Shows the end of `lines`, or further up by `lines_from_bottom`, which is
/// clamped to what there is to scroll.
fn draw_scrolled(
    frame: &mut Frame,
    area: Rect,
    block: Block,
    lines: Vec<Line<'static>>,
    lines_from_bottom: &mut usize,
) {
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = paragraph.line_count(inner.width);
    let max_scroll = total.saturating_sub(inner.height as usize);
    *lines_from_bottom = (*lines_from_bottom).min(max_scroll);
    let top = u16::try_from(max_scroll - *lines_from_bottom).unwrap_or(u16::MAX);
    frame.render_widget(paragraph.scroll((top, 0)).block(block), area);
}

fn change_line(root: &Path, change: &Change) -> Line<'static> {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let (label, color, text) = match change {
        Change::Created(path) => ("created:", Color::Green, relative(path)),
        Change::Changed(path) => ("changed:", Color::Blue, relative(path)),
        Change::Renamed { from, to } => (
            "renamed:",
            Color::Yellow,
            format!("{} -> {}", relative(from), relative(to)),
        ),
        Change::Deleted(path) => ("deleted:", Color::Red, relative(path)),
    };
    Line::from(vec![label.fg(color), Span::raw(format!(" {text}"))])
}
