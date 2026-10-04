use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::agent::Verdict;
use crate::watcher::Change;

const TICK: Duration = Duration::from_millis(100);

pub enum Update {
    Changes(Vec<Change>),
    Verdict(Result<Verdict>),
    WatchErrors(Vec<String>),
}

pub struct Screen {
    root: PathBuf,
    status: Vec<String>,
    log: Vec<Line<'static>>,
    lines_from_bottom: usize,
    quit: bool,
}

impl Screen {
    pub fn new(root: &Path, status: Vec<String>) -> Self {
        Self {
            root: root.to_path_buf(),
            status,
            log: Vec::new(),
            lines_from_bottom: 0,
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
                        self.log.push(Line::from("Stopped watching.".red()));
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_down(1),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::End | KeyCode::Char('G') => self.lines_from_bottom = 0,
            _ => {}
        }
    }

    fn scroll_up(&mut self, lines: usize) {
        self.lines_from_bottom = self.lines_from_bottom.saturating_add(lines);
    }

    fn scroll_down(&mut self, lines: usize) {
        self.lines_from_bottom = self.lines_from_bottom.saturating_sub(lines);
    }

    fn apply(&mut self, update: Update) {
        match update {
            Update::Changes(changes) => {
                for change in &changes {
                    self.log.push(change_line(&self.root, change));
                }
            }
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
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let mut header_lines = vec![Line::from(vec![
            "Watching ".into(),
            self.root.display().to_string().bold(),
        ])];
        header_lines.extend(self.status.iter().cloned().map(Line::from));
        let [header, log, footer] = Layout::vertical([
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

        self.draw_log(frame, log);

        let keys = Line::from(vec![
            " q".bold(),
            " quit  ".into(),
            "↑↓ PgUp PgDn".bold(),
            " scroll  ".into(),
            "End".bold(),
            " latest".into(),
        ])
        .style(Style::new().fg(Color::DarkGray));
        frame.render_widget(keys, footer);
    }

    fn draw_log(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" Changes ");
        if self.log.is_empty() {
            let waiting =
                Paragraph::new("No changes yet.".add_modifier(Modifier::DIM)).block(block);
            frame.render_widget(waiting, area);
            return;
        }
        let inner = block.inner(area);
        let paragraph = Paragraph::new(self.log.clone()).wrap(Wrap { trim: false });
        let total = paragraph.line_count(inner.width);
        let max_scroll = total.saturating_sub(inner.height as usize);
        self.lines_from_bottom = self.lines_from_bottom.min(max_scroll);
        let top = (max_scroll - self.lines_from_bottom) as u16;
        frame.render_widget(paragraph.scroll((top, 0)).block(block), area);
    }
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
