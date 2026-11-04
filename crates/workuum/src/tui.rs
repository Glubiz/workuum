use std::io::{self, IsTerminal};
use std::path::PathBuf;

use clap::{Arg, CommandFactory};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use toml::Value;

use crate::cli::Cli;
use crate::settings::{self, Settings, is_flag, is_list};

const HELP_MIN_WIDTH: u16 = 70;
const NONE: &str = "(none)";

enum Mode {
    Browse,
    Edit(String),
    Name(String),
    Delete,
    Quit,
}

pub struct App {
    path: PathBuf,
    command: clap::Command,
    settings: Settings,
    saved: Settings,
    current: String,
    selected: usize,
    mode: Mode,
    notice: String,
    done: bool,
}

fn on_off(on: bool) -> String {
    match on {
        true => "on".into(),
        false => "off".into(),
    }
}

fn default_text(arg: &Arg) -> String {
    let default = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ");
    match (is_flag(arg), default.is_empty()) {
        (true, _) => on_off(default == "true"),
        (false, true) => NONE.into(),
        (false, false) => default,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Boolean(on) => on_off(*on),
        Value::String(text) => text.clone(),
        Value::Array(items) if items.is_empty() => NONE.into(),
        Value::Array(items) => items.iter().map(value_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

impl App {
    pub fn new(
        path: PathBuf,
        mut settings: Settings,
        mut command: clap::Command,
        session: Option<&str>,
    ) -> Result<App, String> {
        command.build();
        let current = match settings.select(session)? {
            Some((name, _)) => name.clone(),
            None => "default".into(),
        };
        settings.sessions.entry(current.clone()).or_default();
        Ok(App {
            path,
            command,
            saved: settings.clone(),
            settings,
            current,
            selected: 0,
            mode: Mode::Browse,
            notice: String::new(),
            done: false,
        })
    }

    fn arg(&self) -> &Arg {
        settings::options(&self.command)[self.selected]
    }

    fn key(&self) -> String {
        self.arg().get_long().unwrap_or_default().to_string()
    }

    fn set(&self, key: &str) -> Option<&Value> {
        self.settings.sessions[&self.current].get(key)
    }

    fn session_mut(&mut self) -> &mut settings::Session {
        self.settings
            .sessions
            .entry(self.current.clone())
            .or_default()
    }

    /// Why the saved value of `arg` is not accepted, if it is not.
    fn problem(&self, arg: &Arg) -> Option<String> {
        let value = self.set(arg.get_long()?)?;
        settings::texts(&self.command, arg, value).err()
    }

    fn save(&mut self) -> bool {
        match self.settings.save(&self.path) {
            Ok(()) => {
                self.saved = self.settings.clone();
                self.notice = format!("saved {}", self.path.display());
                true
            }
            Err(error) => {
                self.notice = format!("cannot save {}: {error}", self.path.display());
                false
            }
        }
    }

    fn request_quit(&mut self) {
        match self.settings == self.saved {
            true => self.done = true,
            false => self.mode = Mode::Quit,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers == KeyModifiers::CONTROL;
        if ctrl && key.code == KeyCode::Char('c') {
            return self.request_quit();
        }
        match &mut self.mode {
            Mode::Browse => self.browse(key.code),
            Mode::Edit(input) | Mode::Name(input) => match key.code {
                KeyCode::Char(c) => input.push(c),
                KeyCode::Backspace => drop(input.pop()),
                KeyCode::Esc => {
                    self.mode = Mode::Browse;
                    self.notice.clear();
                }
                KeyCode::Enter => self.accept(),
                _ => {}
            },
            Mode::Delete => match key.code {
                KeyCode::Char('y') => self.delete_session(),
                KeyCode::Char('n') | KeyCode::Esc => self.mode = Mode::Browse,
                _ => {}
            },
            Mode::Quit => match key.code {
                KeyCode::Char('s') => self.done = self.save(),
                KeyCode::Char('d') => self.done = true,
                KeyCode::Char('c') | KeyCode::Esc => self.mode = Mode::Browse,
                _ => {}
            },
        }
    }

    fn browse(&mut self, code: KeyCode) {
        let count = settings::options(&self.command).len();
        let names: Vec<String> = self.settings.sessions.keys().cloned().collect();
        let at = names
            .iter()
            .position(|name| *name == self.current)
            .unwrap_or(0);
        self.notice.clear();
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = (self.selected + count - 1) % count,
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1) % count,
            KeyCode::Left => self.current = names[(at + names.len() - 1) % names.len()].clone(),
            KeyCode::Right => self.current = names[(at + 1) % names.len()].clone(),
            KeyCode::Char('r') => {
                let key = self.key();
                self.session_mut().remove(&key);
            }
            KeyCode::Char('s') => {
                self.save();
            }
            KeyCode::Char('n') => self.mode = Mode::Name(String::new()),
            KeyCode::Char('d') if names.len() == 1 => {
                self.notice = "cannot delete the only session".into();
            }
            KeyCode::Char('d') => self.mode = Mode::Delete,
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            KeyCode::Char(' ') if is_flag(self.arg()) => self.toggle(),
            KeyCode::Enter if is_flag(self.arg()) => self.toggle(),
            KeyCode::Enter => {
                let text = match self.set(&self.key()) {
                    Some(value) => value_text(value),
                    None => default_text(self.arg()),
                };
                self.mode = Mode::Edit(text.replace(NONE, ""));
            }
            _ => {}
        }
    }

    fn toggle(&mut self) {
        let key = self.key();
        let on = self.set(&key).and_then(Value::as_bool).unwrap_or(false);
        self.session_mut().insert(key, Value::Boolean(!on));
    }

    fn delete_session(&mut self) {
        self.settings.sessions.remove(&self.current);
        self.current = self
            .settings
            .sessions
            .keys()
            .next()
            .cloned()
            .unwrap_or_default();
        self.mode = Mode::Browse;
    }

    // Enter in the edit or name prompt; invalid input leaves the prompt open.
    fn accept(&mut self) {
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::Name(name) => self.create_session(name.trim().to_string()),
            Mode::Edit(input) => self.set_value(input),
            other => self.mode = other,
        }
    }

    fn create_session(&mut self, name: String) {
        if name.is_empty() || self.settings.sessions.contains_key(&name) {
            self.notice = format!("'{name}' is empty or already a session");
            self.mode = Mode::Name(name);
            return;
        }
        let copy = self.settings.sessions[&self.current].clone();
        self.settings.sessions.insert(name.clone(), copy);
        self.current = name;
        self.notice.clear();
    }

    fn set_value(&mut self, input: String) {
        let (key, arg) = (self.key(), self.arg().clone());
        let list = is_list(&arg);
        let mut items: Vec<String> = match list {
            true => input
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(String::from)
                .collect(),
            false => vec![input.trim().to_string()],
        };
        if let Some(error) = items
            .iter()
            .find_map(|item| settings::check(&self.command, &arg, item).err())
        {
            self.notice = error;
            self.mode = Mode::Edit(input);
            return;
        }
        self.notice.clear();
        if items.is_empty() {
            self.session_mut().remove(&key);
            return;
        }
        let value = match list {
            true => Value::Array(items.into_iter().map(Value::String).collect()),
            false => Value::String(items.swap_remove(0)),
        };
        self.session_mut().insert(key, value);
    }

    pub fn draw(&self, frame: &mut Frame) {
        let dirty = if self.settings == self.saved {
            ""
        } else {
            " (unsaved)"
        };
        let outer = Block::bordered().title(format!(
            " workuum settings -- session: {}{dirty} ",
            self.current
        ));
        let inner = outer.inner(frame.area());
        frame.render_widget(outer, frame.area());
        let [tabs, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(inner);

        let mut spans = vec![Span::raw("Sessions:  ")];
        for name in self.settings.sessions.keys() {
            let span = match *name == self.current {
                true => Span::styled(
                    format!("[{name}]"),
                    Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
                ),
                false => Span::raw(name.clone()),
            };
            spans.extend([span, Span::raw("   ")]);
        }
        spans.push(Span::styled("+ new", Style::new().fg(Color::DarkGray)));
        frame.render_widget(Line::from(spans), tabs);

        let wide = body.width >= HELP_MIN_WIDTH;
        let [table_area, help_area] = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(if wide { 32 } else { 0 }),
        ])
        .areas(body);
        let editing = match &self.mode {
            Mode::Edit(input) => Some(input),
            _ => None,
        };
        let options = settings::options(&self.command);
        let rows = options.iter().enumerate().map(|(index, arg)| {
            let key = arg.get_long().unwrap_or_default();
            let problem = self.problem(arg).is_some();
            let (mut value, mut style) = match (self.set(key), editing) {
                (_, Some(input)) if index == self.selected => {
                    (format!("{input}_"), Style::new().fg(Color::Yellow))
                }
                (Some(value), _) => (value_text(value), Style::new()),
                (None, _) => (default_text(arg), Style::new().fg(Color::DarkGray)),
            };
            if self.set(key).is_some() && editing.is_none() || problem {
                value.push_str(if problem { " !" } else { " *" });
                style = Style::new().fg(if problem { Color::Red } else { Color::Reset });
            }
            Row::new([
                Cell::from(key),
                Cell::from(value).style(style),
                Cell::from(default_text(arg)).style(Style::new().fg(Color::DarkGray)),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(19),
                Constraint::Min(12),
                Constraint::Length(10),
            ],
        )
        .header(
            Row::new(["Option", "Value", "Default"])
                .style(Style::new().add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
        frame.render_stateful_widget(
            table,
            table_area,
            &mut TableState::default().with_selected(self.selected),
        );

        if wide {
            let arg = self.arg();
            let mut lines = vec![Line::from(
                arg.get_help()
                    .map(|help| help.to_string())
                    .unwrap_or_default(),
            )];
            if let Some(problem) = self.problem(arg) {
                lines.extend([
                    Line::raw(""),
                    Line::styled(problem, Style::new().fg(Color::Red)),
                ]);
            }
            let pane = Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(Block::bordered().title(format!(" {} ", self.key())));
            frame.render_widget(pane, help_area);
        }

        let hint = match &self.mode {
            Mode::Browse => {
                "↑↓ move  ⏎ edit  r reset  ←→ session  n new  d delete  s save  q quit".to_string()
            }
            Mode::Edit(_) if is_list(self.arg()) => {
                "comma-separated values  enter accept  esc cancel".into()
            }
            Mode::Edit(_) => "enter accept  esc cancel".into(),
            Mode::Name(name) => format!(
                "new session (copy of {}): {name}_  enter create  esc cancel",
                self.current
            ),
            Mode::Delete => format!("delete session '{}'? y/n", self.current),
            Mode::Quit => "unsaved changes: s save  d discard  c cancel".into(),
        };
        let notice = match (self.notice.is_empty(), wide) {
            (false, _) => self.notice.clone(),
            (true, false) => self.problem(self.arg()).unwrap_or_default(),
            (true, true) => String::new(),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(notice, Style::new().fg(Color::Red)),
                Line::raw(hint),
            ]),
            footer,
        );
    }
}

pub fn run(session: Option<&str>) -> Result<(), String> {
    let path = settings::path()?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(format!(
            "workuum settings needs an interactive terminal; edit {} instead",
            path.display()
        ));
    }
    let mut app = App::new(
        path.clone(),
        Settings::load(&path)?,
        Cli::command(),
        session,
    )?;
    ratatui::run(|terminal| {
        while !app.done {
            terminal.draw(|frame| app.draw(frame))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.handle_key(key);
            }
        }
        Ok(())
    })
    .map_err(|error: io::Error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tempfile::TempDir;

    fn app(toml: &str) -> (App, TempDir) {
        let dir = TempDir::new().unwrap();
        let settings = toml::from_str(toml).unwrap();
        let app = App::new(
            dir.path().join("settings.toml"),
            settings,
            Cli::command(),
            None,
        )
        .unwrap();
        (app, dir)
    }

    fn press(app: &mut App, codes: &[KeyCode]) {
        for code in codes {
            app.handle_key(KeyEvent::from(*code));
        }
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, &[KeyCode::Char(c)]);
        }
    }

    fn select(app: &mut App, key: &str) {
        while app.key() != key {
            press(app, &[KeyCode::Down]);
        }
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal.backend().to_string()
    }

    fn edit(app: &mut App, key: &str, text: &str) {
        select(app, key);
        press(app, &[KeyCode::Enter]);
        for _ in 0..20 {
            press(app, &[KeyCode::Backspace]);
        }
        type_text(app, text);
        press(app, &[KeyCode::Enter]);
    }

    #[test]
    fn valid_edit_marks_the_value_as_set() {
        let (mut app, _dir) = app("");

        edit(&mut app, "min-free", "15%");

        assert_eq!(
            app.settings.sessions["default"]["min-free"],
            Value::String("15%".into())
        );
        let screen = render(&app, 80, 24);
        assert!(screen.contains("15% *"), "{screen}");
        assert!(screen.contains("10%"), "{screen}");
        assert!(screen.contains("Free space to keep"), "{screen}");
    }

    #[test]
    fn invalid_edit_shows_the_parser_message_and_changes_nothing() {
        let (mut app, _dir) = app("");

        edit(&mut app, "min-free", "lots");

        assert!(matches!(app.mode, Mode::Edit(_)));
        assert!(app.settings.sessions["default"].is_empty());
        assert!(app.notice.contains("invalid size"), "{}", app.notice);
        assert!(render(&app, 80, 24).contains("invalid size"));
    }

    #[test]
    fn list_edit_is_comma_separated_and_reset_removes_the_key() {
        let (mut app, _dir) = app("");

        edit(&mut app, "root", "~/code, /srv");
        let roots = Value::Array(vec![
            Value::String("~/code".into()),
            Value::String("/srv".into()),
        ]);
        assert_eq!(app.settings.sessions["default"]["root"], roots);

        press(&mut app, &[KeyCode::Char('r')]);
        assert!(app.settings.sessions["default"].is_empty());
    }

    #[test]
    fn enter_and_space_toggle_a_boolean() {
        let (mut app, _dir) = app("");
        select(&mut app, "fetch");

        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(
            app.settings.sessions["default"]["fetch"],
            Value::Boolean(true)
        );
        press(&mut app, &[KeyCode::Char(' ')]);
        assert_eq!(
            app.settings.sessions["default"]["fetch"],
            Value::Boolean(false)
        );
    }

    #[test]
    fn new_session_copies_the_current_one_and_delete_removes_it() {
        let (mut app, _dir) = app("[sessions.default]\nmin-free = \"15%\"\n");

        press(&mut app, &[KeyCode::Char('n')]);
        type_text(&mut app, "aggressive");
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.current, "aggressive");
        assert_eq!(
            app.settings.sessions["aggressive"]["min-free"],
            Value::String("15%".into())
        );
        assert!(render(&app, 80, 24).contains("[aggressive]"));

        press(&mut app, &[KeyCode::Left]);
        assert_eq!(app.current, "default");
        press(&mut app, &[KeyCode::Right]);
        assert_eq!(app.current, "aggressive");
        press(&mut app, &[KeyCode::Char('d'), KeyCode::Char('n')]);
        assert!(app.settings.sessions.contains_key("aggressive"));
        press(&mut app, &[KeyCode::Char('d'), KeyCode::Char('y')]);
        assert!(!app.settings.sessions.contains_key("aggressive"));
        assert_eq!(app.current, "default");
    }

    #[test]
    fn save_writes_the_expected_toml() {
        let (mut app, dir) = app("");
        edit(&mut app, "min-free", "15%");
        edit(&mut app, "root", "~/code");
        select(&mut app, "fetch");
        press(&mut app, &[KeyCode::Enter, KeyCode::Char('s')]);

        let text = std::fs::read_to_string(dir.path().join("settings.toml")).unwrap();

        assert_eq!(
            text,
            "[sessions.default]\nfetch = true\nmin-free = \"15%\"\nroot = [\"~/code\"]\n"
        );
        assert!(app.settings == app.saved);
    }

    #[test]
    fn quitting_with_unsaved_changes_prompts() {
        let (mut app, dir) = app("");
        edit(&mut app, "min-free", "15%");

        press(&mut app, &[KeyCode::Char('q')]);
        assert!(!app.done && matches!(app.mode, Mode::Quit));
        assert!(render(&app, 80, 24).contains("s save  d discard  c cancel"));
        press(&mut app, &[KeyCode::Char('c')]);
        assert!(!app.done);
        press(&mut app, &[KeyCode::Char('q'), KeyCode::Char('d')]);
        assert!(app.done);
        assert!(!dir.path().join("settings.toml").exists());
    }

    #[test]
    fn quitting_without_changes_exits_at_once() {
        let (mut app, _dir) = app("");

        press(&mut app, &[KeyCode::Esc]);

        assert!(app.done);
    }

    #[test]
    fn an_invalid_saved_value_is_shown_and_can_be_fixed() {
        let (mut app, _dir) = app("[sessions.default]\nmin-free = \"lots\"\n");
        select(&mut app, "min-free");

        let screen = render(&app, 80, 24);
        assert!(screen.contains("lots !"), "{screen}");
        assert!(screen.contains("invalid size"), "{screen}");
        edit(&mut app, "min-free", "20GB");
        assert!(app.problem(app.arg()).is_none());
    }

    #[test]
    fn narrow_terminals_hide_the_help_pane() {
        let (app, _dir) = app("");

        let screen = render(&app, 60, 24);

        assert!(!screen.contains("Free space"), "{screen}");
        assert!(screen.contains("min-free"), "{screen}");
    }
}
