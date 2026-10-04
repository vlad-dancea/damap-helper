use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use super::model_panel;
use crate::agent::Trace;

/// Oldest entries go first once the pane holds this many, so a long session
/// does not slow down drawing.
const MAX_ENTRIES: usize = 1_000;
const GUTTER: &str = "  │ ";

enum Entry {
    /// Starts a review: a rule across the pane.
    Review(String),
    Step {
        header: Vec<Span<'static>>,
        color: Color,
        body: Vec<BodyLine>,
        /// Whether a blank line separates it from what follows.
        spaced: bool,
    },
}

/// A logical line of a step's body; wrapped to the pane when drawn, with
/// continuation lines indented under the text, past `key`.
struct BodyLine {
    key: Option<String>,
    text: String,
    style: Style,
}

impl BodyLine {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            key: None,
            text: text.into(),
            style,
        }
    }

    fn keyed(key: &str, text: impl Into<String>) -> Self {
        Self {
            key: Some(format!("{key}: ")),
            text: text.into(),
            style: Style::new(),
        }
    }
}

#[derive(Default)]
pub struct DebugLog {
    entries: Vec<Entry>,
    rendered: Option<(u16, Vec<Line<'static>>)>,
}

impl DebugLog {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn review_started(&mut self) {
        let time = jiff::Zoned::now().strftime("%H:%M:%S").to_string();
        self.push(Entry::Review(time));
    }

    pub fn add(&mut self, trace: Trace) {
        let entry = match trace {
            Trace::Sent { role, text } => {
                let (title, style) = if role == "system" {
                    ("system prompt", Style::new().add_modifier(Modifier::DIM))
                } else {
                    ("prompt", Style::new())
                };
                step(sent(title), Color::Cyan, prompt_lines(&text, style))
            }
            Trace::Request {
                turn,
                choice,
                tools,
            } => {
                let header = vec![
                    "→ ".cyan().bold(),
                    format!("request {turn}").cyan().bold(),
                    format!("  {}", model_panel::describe(&choice)).add_modifier(Modifier::DIM),
                ];
                let mut body = Vec::new();
                if turn == 1 {
                    let mut tools = BodyLine::keyed("tools", tools.join(", "));
                    tools.style = tools.style.add_modifier(Modifier::DIM);
                    body.push(tools);
                }
                Entry::Step {
                    header,
                    color: Color::Cyan,
                    body,
                    spaced: false,
                }
            }
            Trace::Reply {
                turn,
                reasoning,
                texts,
                calls,
                stop_reason,
                tokens_in,
                tokens_out,
            } => {
                let count = |tokens: Option<i32>| tokens.map_or("?".to_string(), |n| n.to_string());
                let mut details = format!(
                    "  {} in · {} out tokens",
                    count(tokens_in),
                    count(tokens_out)
                );
                if let Some(reason) = stop_reason {
                    details.push_str(&format!(" · {reason}"));
                }
                let header = vec![
                    "← ".green().bold(),
                    format!("reply {turn}").green().bold(),
                    details.add_modifier(Modifier::DIM),
                ];
                let mut body = Vec::new();
                if let Some(reasoning) = reasoning.filter(|r| !r.trim().is_empty()) {
                    body.push(BodyLine::new("thinking", Style::new().fg(Color::Magenta)));
                    let style = Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC);
                    body.extend(text_lines(&reasoning, style));
                }
                for text in texts.iter().filter(|t| !t.trim().is_empty()) {
                    gap(&mut body);
                    body.extend(text_lines(text, Style::new()));
                }
                for (name, arguments) in calls {
                    gap(&mut body);
                    body.push(BodyLine::new(
                        format!("⚙ {name}"),
                        Style::new().fg(Color::Yellow).bold(),
                    ));
                    body.extend(argument_lines(&arguments));
                }
                if body.is_empty() {
                    body.push(BodyLine::new(
                        "(empty reply)",
                        Style::new().add_modifier(Modifier::DIM),
                    ));
                }
                step(header, Color::Green, body)
            }
            Trace::ToolResult { name, output } => {
                let body = if output.is_empty() {
                    vec![BodyLine::new(
                        "(empty)",
                        Style::new().add_modifier(Modifier::DIM),
                    )]
                } else {
                    text_lines(&output, Style::new())
                };
                step(sent(&format!("{name} result")), Color::Cyan, body)
            }
            Trace::Failed(e) => step(
                vec!["✗ ".red().bold(), "request failed".red().bold()],
                Color::Red,
                text_lines(&e, Style::new().fg(Color::Red)),
            ),
        };
        self.push(entry);
    }

    /// The log as lines that fit `width` columns.
    pub fn lines(&mut self, width: u16) -> &[Line<'static>] {
        if self.rendered.as_ref().is_none_or(|(w, _)| *w != width) {
            let mut lines = Vec::new();
            let mut previous_spaced = false;
            for entry in &self.entries {
                if previous_spaced {
                    lines.push(Line::default());
                }
                render(entry, width as usize, &mut lines);
                previous_spaced = !matches!(entry, Entry::Step { spaced: false, .. });
            }
            self.rendered = Some((width, lines));
        }
        &self.rendered.as_ref().expect("rendered above").1
    }

    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.drain(..self.entries.len() - MAX_ENTRIES);
        }
        self.rendered = None;
    }
}

fn step(header: Vec<Span<'static>>, color: Color, body: Vec<BodyLine>) -> Entry {
    Entry::Step {
        header,
        color,
        body,
        spaced: true,
    }
}

fn sent(title: &str) -> Vec<Span<'static>> {
    vec!["→ ".cyan().bold(), title.to_string().cyan().bold()]
}

/// A blank line between parts of a body, but not before the first.
fn gap(body: &mut Vec<BodyLine>) {
    if !body.is_empty() {
        body.push(BodyLine::new("", Style::new()));
    }
}

fn text_lines(text: &str, style: Style) -> Vec<BodyLine> {
    text.trim_end()
        .lines()
        .map(|line| BodyLine::new(line, style))
        .collect()
}

/// Like `text_lines`, but dims fenced code blocks such as the DMP's JSON.
fn prompt_lines(text: &str, style: Style) -> Vec<BodyLine> {
    let code = Style::new().add_modifier(Modifier::DIM);
    let mut in_code = false;
    text.trim_end()
        .lines()
        .map(|line| {
            let fence = line.trim_start().starts_with("```");
            let line_style = if in_code || fence { code } else { style };
            if fence {
                in_code = !in_code;
            }
            BodyLine::new(line, line_style)
        })
        .collect()
}

/// Tool arguments as `key: value` lines; strings unquoted, so explanations
/// read as text.
fn argument_lines(arguments: &Value) -> Vec<BodyLine> {
    let Value::Object(fields) = arguments else {
        let pretty = serde_json::to_string_pretty(arguments).unwrap_or_default();
        return text_lines(&pretty, Style::new());
    };
    if fields.is_empty() {
        return vec![BodyLine::new(
            "(no arguments)",
            Style::new().add_modifier(Modifier::DIM),
        )];
    }
    let mut lines = Vec::new();
    for (key, value) in fields {
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let mut parts = text.lines();
        lines.push(BodyLine::keyed(key, parts.next().unwrap_or_default()));
        let indent = " ".repeat(key.len() + 2);
        lines.extend(parts.map(|part| BodyLine::new(format!("{indent}{part}"), Style::new())));
    }
    lines
}

fn render(entry: &Entry, width: usize, lines: &mut Vec<Line<'static>>) {
    match entry {
        Entry::Review(time) => {
            let label = format!("── review at {time} ");
            let rest = width.saturating_sub(label.chars().count());
            lines.push(Line::from(
                format!("{label}{}", "─".repeat(rest))
                    .fg(Color::Magenta)
                    .bold(),
            ));
        }
        Entry::Step {
            header,
            color,
            body,
            ..
        } => {
            lines.push(Line::from(header.clone()));
            let gutter = Span::styled(GUTTER, Style::new().fg(*color).add_modifier(Modifier::DIM));
            let room = width.saturating_sub(GUTTER.chars().count()).max(10);
            for body_line in body {
                for (i, piece) in wrap(body_line, room).into_iter().enumerate() {
                    let mut spans = vec![gutter.clone()];
                    match &body_line.key {
                        Some(key) if i == 0 && piece.starts_with(key.as_str()) => {
                            spans.push(Span::styled(key.clone(), Style::new().bold()));
                            spans.push(Span::styled(
                                piece[key.len()..].to_string(),
                                body_line.style,
                            ));
                        }
                        _ => spans.push(Span::styled(piece, body_line.style)),
                    }
                    lines.push(Line::from(spans));
                }
            }
        }
    }
}

/// Wraps at spaces where it can, mid-word where it must; continuation lines
/// are indented like the start of the text, after the key if there is one.
fn wrap(line: &BodyLine, width: usize) -> Vec<String> {
    let key = line.key.as_deref().unwrap_or_default();
    let leading = line.text.len() - line.text.trim_start().len();
    let mut wrapper = Wrapper {
        width,
        indent: (key.len() + leading).min(width / 2),
        pieces: Vec::new(),
        current: String::new(),
        current_width: 0,
    };
    let mut word = String::new();
    for c in key.chars().chain(line.text.chars()) {
        if c == ' ' && !word.trim().is_empty() {
            wrapper.push_word(&word);
            word.clear();
        }
        word.push(c);
    }
    wrapper.push_word(&word);
    wrapper.pieces.push(wrapper.current);
    wrapper.pieces
}

struct Wrapper {
    width: usize,
    indent: usize,
    pieces: Vec<String>,
    current: String,
    current_width: usize,
}

impl Wrapper {
    /// `word` comes with the spaces before it, dropped if it starts a line.
    fn push_word(&mut self, mut word: &str) {
        let word_width: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
        if self.current_width + word_width > self.width && !self.current.trim().is_empty() {
            self.break_line();
            word = word.trim_start();
        }
        for c in word.chars() {
            let c_width = c.width().unwrap_or(0);
            if self.current_width + c_width > self.width && self.current_width > self.indent {
                self.break_line();
            }
            self.current.push(c);
            self.current_width += c_width;
        }
    }

    fn break_line(&mut self) {
        let done = std::mem::replace(&mut self.current, " ".repeat(self.indent));
        self.pieces.push(done.trim_end().to_string());
        self.current_width = self.indent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped(text: &str, width: usize) -> Vec<String> {
        wrap(&BodyLine::new(text, Style::new()), width)
    }

    #[test]
    fn wraps_under_the_start_of_the_text() {
        assert_eq!(
            wrapped("  one two three four", 10),
            ["  one two", "  three", "  four"]
        );
        let keyed = BodyLine::keyed("path", "a b c d e");
        assert_eq!(wrap(&keyed, 14), ["path: a b c d", "      e"]);
    }

    #[test]
    fn breaks_words_longer_than_a_line() {
        assert_eq!(wrapped("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn keeps_short_lines_and_blank_lines() {
        assert_eq!(wrapped("short", 10), ["short"]);
        assert_eq!(wrapped("", 10), [""]);
    }
}
