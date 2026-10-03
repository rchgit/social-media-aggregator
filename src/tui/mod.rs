use std::path::PathBuf;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::config::Config;
use crate::error::Result;
use crate::images;
use crate::models::{Account, Network, Post, Topic};
use crate::storage::Storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Accounts,
    Topics,
    Feed,
    Sync,
}

struct Input {
    prompt: String,
    buffer: String,
}

struct Detail {
    post: Post,
    /// Rendered ANSI-free previews, one per image (empty when none decoded).
    previews: Vec<images::Preview>,
}

struct App {
    storage: Storage,
    config: Config,
    client: reqwest::blocking::Client,
    screen: Screen,
    accounts: Vec<Account>,
    topics: Vec<Topic>,
    topic_sel: usize,
    feed: Vec<Post>,
    feed_sel: usize,
    feed_offset: usize,
    list_sel: usize,
    detail: Option<Detail>,
    input: Option<Input>,
    sync_log: Vec<String>,
    message: String,
    should_quit: bool,
}

impl App {
    fn new(config: &Config, storage: Storage) -> Result<Self> {
        let accounts = storage.list_accounts()?;
        let topics = storage.list_topics()?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("sma/0.1")
            .build()?;
        let mut app = Self {
            storage,
            config: config.clone(),
            client,
            screen: Screen::Feed,
            accounts,
            topics,
            topic_sel: 0,
            feed: Vec::new(),
            feed_sel: 0,
            feed_offset: 0,
            list_sel: 0,
            detail: None,
            input: None,
            sync_log: Vec::new(),
            message: String::new(),
            should_quit: false,
        };
        app.refresh_feed()?;
        Ok(app)
    }

    fn refresh_feed(&mut self) -> Result<()> {
        let Some(topic) = self.topics.get(self.topic_sel) else {
            self.feed.clear();
            return Ok(());
        };
        let page = self.config.settings.page_size;
        self.feed = self.storage.feed(topic.id, page, self.feed_offset)?;
        self.feed_sel = self.feed_sel.min(self.feed.len().saturating_sub(1));
        Ok(())
    }

    fn refresh_accounts(&mut self) -> Result<()> {
        self.accounts = self.storage.list_accounts()?;
        self.list_sel = self.list_sel.min(self.accounts.len().saturating_sub(1));
        Ok(())
    }

    fn refresh_topics(&mut self) -> Result<()> {
        self.topics = self.storage.list_topics()?;
        self.topic_sel = self.topic_sel.min(self.topics.len().saturating_sub(1));
        self.list_sel = self.list_sel.min(self.topics.len().saturating_sub(1));
        Ok(())
    }

    fn run_sync(&mut self) {
        let secrets = self.config.secret_resolver();
        let report = crate::sync::sync(&self.storage, &secrets, None);
        self.sync_log.clear();
        match report {
            Ok(results) => {
                for r in results {
                    let status = match &r.status {
                        crate::sync::SyncStatus::Ok => "ok".to_string(),
                        crate::sync::SyncStatus::Skipped => "skipped".to_string(),
                        crate::sync::SyncStatus::CredentialsRequired(s) => {
                            format!("missing credentials: {s}")
                        }
                        crate::sync::SyncStatus::RateLimited(secs) => {
                            format!("rate limited, retry in {secs}s")
                        }
                        crate::sync::SyncStatus::Error(e) => format!("error: {e}"),
                    };
                    self.sync_log.push(format!(
                        "{} {}: fetched={} new={} {status}",
                        r.network, r.handle, r.fetched, r.inserted
                    ));
                }
            }
            Err(e) => self.sync_log.push(format!("sync failed: {e}")),
        }
        let _ = self.refresh_accounts();
    }

    fn open_detail(&mut self, post: Post) {
        let previews = post
            .images
            .iter()
            .filter_map(|img| {
                let path = img.local_path.as_deref().map(PathBuf::from).or_else(|| {
                    images::download_image(&self.client, &img.url, &self.config.cache_dir()).ok().flatten()
                })?;
                images::preview(&path, 80, 24).ok()
            })
            .collect();
        self.detail = Some(Detail { post, previews });
    }

    fn commit_input(&mut self) {
        let Some(input) = self.input.take() else {
            return;
        };
        let line = input.buffer.trim().to_string();
        if line.is_empty() {
            return;
        }
        let result = match self.screen {
            Screen::Accounts => self.add_account(&line),
            Screen::Topics => self.add_topic(&line),
            _ => Ok(()),
        };
        self.message = match result {
            Ok(()) => "added".to_string(),
            Err(e) => format!("error: {e}"),
        };
    }

    fn add_account(&mut self, line: &str) -> Result<()> {
        let parts: Vec<&str> = line.split(':').map(str::trim).collect();
        let (network, handle) = match parts.as_slice() {
            [network, handle, ..] if !network.is_empty() && !handle.is_empty() => (*network, *handle),
            _ => {
                return Err(crate::Error::InvalidArgument(
                    "expected NETWORK:HANDLE[:DISPLAY[:SECRET]]".into(),
                ))
            }
        };
        let network: Network = network
            .parse()
            .map_err(|_| crate::Error::InvalidArgument(format!("unknown network `{network}`")))?;
        let display = parts.get(2).copied().unwrap_or(handle);
        let secret = parts.get(3).copied().unwrap_or("");
        let display = if display.is_empty() { handle } else { display };
        self.storage.add_account(network, handle, display, secret)?;
        self.refresh_accounts()
    }

    fn add_topic(&mut self, line: &str) -> Result<()> {
        let (name, kws) = match line.split_once(':') {
            Some((n, k)) => (n.trim(), k.trim()),
            None => (line.trim(), ""),
        };
        if name.is_empty() {
            return Err(crate::Error::InvalidArgument("topic name is empty".into()));
        }
        let keywords: Vec<String> = if kws.is_empty() {
            Vec::new()
        } else {
            kws.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
        };
        self.storage.add_topic(name, &keywords)?;
        self.refresh_topics()
    }

    fn on_key(&mut self, key: event::KeyEvent) {
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        if self.detail.is_some() {
            if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                self.detail = None;
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => {
                self.screen = Screen::Accounts;
                let _ = self.refresh_accounts();
            }
            KeyCode::Char('2') => {
                self.screen = Screen::Topics;
                let _ = self.refresh_topics();
            }
            KeyCode::Char('3') => {
                self.screen = Screen::Feed;
                let _ = self.refresh_feed();
            }
            KeyCode::Char('4') => {
                self.screen = Screen::Sync;
                self.sync_log = vec!["press s to sync".into()];
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('a') => self.start_add(),
            KeyCode::Char('d') => self.delete_selected(),
            KeyCode::Char('e') => self.toggle_selected(),
            KeyCode::Char('t') if self.screen == Screen::Feed => {
                if !self.topics.is_empty() {
                    self.topic_sel = (self.topic_sel + 1) % self.topics.len();
                    self.feed_offset = 0;
                    self.feed_sel = 0;
                    let _ = self.refresh_feed();
                }
            }
            KeyCode::Char('n') if self.screen == Screen::Feed => {
                self.feed_offset += self.config.settings.page_size;
                let _ = self.refresh_feed();
            }
            KeyCode::Char('p') if self.screen == Screen::Feed => {
                self.feed_offset = self.feed_offset.saturating_sub(self.config.settings.page_size);
                let _ = self.refresh_feed();
            }
            KeyCode::Char('s') if self.screen == Screen::Sync => self.run_sync(),
            KeyCode::Enter if self.screen == Screen::Feed => {
                if let Some(post) = self.feed.get(self.feed_sel) {
                    self.open_detail(post.clone());
                }
            }
            _ => {}
        }
    }

    fn on_input_key(&mut self, key: event::KeyEvent) {
        let Some(input) = self.input.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.input = None;
            }
            KeyCode::Enter => self.commit_input(),
            KeyCode::Backspace => {
                input.buffer.pop();
            }
            KeyCode::Char(c) => input.buffer.push(c),
            _ => {}
        }
    }

    fn move_down(&mut self) {
        let len = match self.screen {
            Screen::Accounts => self.accounts.len(),
            Screen::Topics => self.topics.len(),
            Screen::Feed => self.feed.len(),
            Screen::Sync => 0,
        };
        if len > 0 {
            let sel = match self.screen {
                Screen::Feed => &mut self.feed_sel,
                _ => &mut self.list_sel,
            };
            *sel = (*sel + 1) % len;
        }
    }

    fn move_up(&mut self) {
        let len = match self.screen {
            Screen::Accounts => self.accounts.len(),
            Screen::Topics => self.topics.len(),
            Screen::Feed => self.feed.len(),
            Screen::Sync => 0,
        };
        if len > 0 {
            let sel = match self.screen {
                Screen::Feed => &mut self.feed_sel,
                _ => &mut self.list_sel,
            };
            *sel = if *sel == 0 { len - 1 } else { *sel - 1 };
        }
    }

    fn start_add(&mut self) {
        match self.screen {
            Screen::Accounts => {
                self.input = Some(Input {
                    prompt: "add account NETWORK:HANDLE[:DISPLAY[:SECRET]]".into(),
                    buffer: String::new(),
                });
            }
            Screen::Topics => {
                self.input = Some(Input {
                    prompt: "add topic NAME:kw1,kw2,...".into(),
                    buffer: String::new(),
                });
            }
            _ => {}
        }
    }

    fn delete_selected(&mut self) {
        let result: crate::Result<()> = (|| {
            match self.screen {
                Screen::Accounts => {
                    if let Some(a) = self.accounts.get(self.list_sel) {
                        self.storage.remove_account(a.id)?;
                        self.refresh_accounts()?;
                    }
                }
                Screen::Topics => {
                    if let Some(t) = self.topics.get(self.list_sel) {
                        self.storage.remove_topic(t.id)?;
                        self.refresh_topics()?;
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        self.message = match result {
            Ok(()) => "deleted".into(),
            Err(e) => format!("error: {e}"),
        };
    }

    fn toggle_selected(&mut self) {
        let result = match self.screen {
            Screen::Accounts => self
                .accounts
                .get(self.list_sel)
                .map(|a| self.storage.set_account_enabled(a.id, !a.enabled)),
            Screen::Topics => self
                .topics
                .get(self.list_sel)
                .map(|t| self.storage.set_topic_enabled(t.id, !t.enabled)),
            _ => None,
        };
        match result {
            Some(Ok(_)) => {
                let _ = if self.screen == Screen::Accounts {
                    self.refresh_accounts()
                } else {
                    self.refresh_topics()
                };
                self.message = "toggled".into();
            }
            Some(Err(e)) => self.message = format!("error: {e}"),
            None => self.message = "nothing to toggle".into(),
        }
    }
}

pub fn run(config: &Config) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let storage = Storage::open(&config.db_path())?;
    let mut app = App::new(config, storage)?;

    let res = loop {
        terminal.draw(|f| app.render(f))?;
        if app.should_quit {
            break Ok(());
        }
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

impl App {
    fn render(&self, f: &mut Frame) {
        let area = f.area();
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

        f.render_widget(self.header(), chunks[0]);

        match self.screen {
            Screen::Accounts => self.render_accounts(f, chunks[1]),
            Screen::Topics => self.render_topics(f, chunks[1]),
            Screen::Feed => self.render_feed(f, chunks[1]),
            Screen::Sync => self.render_sync(f, chunks[1]),
        }

        let status = if !self.message.is_empty() {
            self.message.as_str()
        } else {
            self.footer_hint()
        };
        f.render_widget(
            Paragraph::new(status).style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );

        if let Some(detail) = &self.detail {
            self.render_detail(f, area, detail);
        }
        if let Some(input) = &self.input {
            self.render_input(f, area, input);
        }
    }

    fn header(&self) -> Paragraph<'static> {
        let title = format!(
            " social media aggregator — {} ",
            match self.screen {
                Screen::Accounts => "accounts",
                Screen::Topics => "topics",
                Screen::Feed => "feed",
                Screen::Sync => "sync",
            }
        );
        let tabs = " [1]accounts [2]topics [3]feed [4]sync  q:quit ";
        Paragraph::new(Line::from(vec![
            Span::styled(title, Style::default().fg(Color::Black).bg(Color::Cyan)),
            Span::styled(tabs, Style::default().fg(Color::DarkGray)),
        ]))
    }

    fn footer_hint(&self) -> &'static str {
        match self.screen {
            Screen::Accounts => "a:add  d:delete  e:enable/disable  j/k:move",
            Screen::Topics => "a:add  d:delete  e:enable/disable  j/k:move",
            Screen::Feed => "t:topic  n/p:page  enter:detail  j/k:move",
            Screen::Sync => "s:sync",
        }
    }

    fn render_accounts(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .accounts
            .iter()
            .map(|a| {
                let state = if a.enabled { " on" } else { "off" };
                let line = Line::from(vec![
                    Span::raw(format!("{:>4} ", a.id)),
                    Span::styled(
                        format!("{:<10}", a.network.as_str()),
                        Style::default().fg(network_color(a.network)),
                    ),
                    Span::raw(format!("{:<20} {:<16} {state}", a.handle, a.display_name)),
                ]);
                ListItem::new(line)
            })
            .collect();

        let mut state = ListState::default();
        state.select(if items.is_empty() { None } else { Some(self.list_sel) });

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" accounts "))
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_topics(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .topics
            .iter()
            .map(|t| {
                let state = if t.enabled { " on" } else { "off" };
                ListItem::new(format!(
                    "{:>4} {:<20} {state} {}",
                    t.id,
                    t.name,
                    t.keywords.join(", ")
                ))
            })
            .collect();

        let mut state = ListState::default();
        state.select(if items.is_empty() { None } else { Some(self.list_sel) });

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" topics "))
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_feed(&self, f: &mut Frame, area: Rect) {
        let topic_name = self
            .topics
            .get(self.topic_sel)
            .map(|t| t.name.as_str())
            .unwrap_or("(no topics)");

        if self.feed.is_empty() {
            let hint = if self.topics.is_empty() {
                "No topics yet. Press 2 to add one.".to_string()
            } else {
                format!("No posts for topic `{topic_name}`. Press 4 to sync.")
            };
            f.render_widget(
                Paragraph::new(hint)
                    .block(Block::default().borders(Borders::ALL).title(" feed "))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }

        let items: Vec<ListItem> = self
            .feed
            .iter()
            .map(|p| {
                let icon = format!("[{}]", p.network.icon());
                let snippet: String = p.content.chars().take(90).collect();
                let one_line = snippet.replace('\n', " ");
                ListItem::new(Line::from(vec![
                    Span::styled(icon, Style::default().fg(network_color(p.network))),
                    Span::styled(
                        format!(" {} ", p.author),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(one_line),
                ]))
            })
            .collect();

        let mut state = ListState::default();
        state.select(Some(self.feed_sel.min(items.len().saturating_sub(1))));

        let title = format!(" feed · topic `{topic_name}` (page {}) ", self.feed_offset / self.config.settings.page_size + 1);
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(Style::default().bg(Color::DarkGray));
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_sync(&self, f: &mut Frame, area: Rect) {
        let text: Vec<Line> = self.sync_log.iter().map(|s| Line::from(s.as_str())).collect();
        f.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" sync "))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn render_detail(&self, f: &mut Frame, area: Rect, detail: &Detail) {
        let p = &detail.post;
        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(
                    format!("{} @{}", p.author, p.account_handle),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "  {}  {}",
                    p.network.as_str(),
                    p.created_at.format("%Y-%m-%d %H:%M:%S")
                )),
            ]),
            Line::from(""),
        ];

        for line in p.content.lines() {
            lines.push(Line::from(line.to_string()));
        }

        if !p.url.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(osc8(&p.url, &p.url)));
        }

        for (i, preview) in detail.previews.iter().enumerate() {
            lines.push(Line::from(format!(" image {}/{} ", i + 1, detail.previews.len())));
            for row in &preview.rows {
                let spans: Vec<Span> = row
                    .iter()
                    .map(|cell| {
                        Span::styled(
                            "▀",
                            Style::default()
                                .fg(Color::Rgb(cell.fg[0], cell.fg[1], cell.fg[2]))
                                .bg(Color::Rgb(cell.bg[0], cell.bg[1], cell.bg[2])),
                        )
                    })
                    .collect();
                lines.push(Line::from(spans));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from("q/esc: back"));

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" post ")
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), area);
    }

    fn render_input(&self, f: &mut Frame, area: Rect, input: &Input) {
        let popup = centered_rect(area, 70, 3);
        let text = format!("{}: {}", input.prompt, input.buffer);
        f.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" input "))
                .style(Style::default().fg(Color::Yellow)),
            popup,
        );
    }
}

fn network_color(network: Network) -> Color {
    match network {
        Network::Facebook => Color::Blue,
        Network::Reddit => Color::Rgb(255, 69, 0),
        Network::X => Color::Gray,
        Network::Instagram => Color::Magenta,
        Network::Threads => Color::LightMagenta,
        Network::TikTok => Color::Cyan,
    }
}

fn osc8(url: &str, label: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{label}\x1b]8;;\x1b\\")
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let w = (area.width * percent_x / 100).max(20).min(area.width);
    let h = (area.height * percent_y / 100).max(3).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn app_with(storage: Storage) -> App {
        let mut config = Config::default();
        config.settings.page_size = 10;
        App::new(&config, storage).unwrap()
    }

    fn key(code: KeyCode) -> event::KeyEvent {
        event::KeyEvent::new(code, event::KeyModifiers::empty())
    }

    fn seeded_app() -> App {
        let storage = Storage::in_memory().unwrap();
        let t = storage.add_topic("tech", &["rust".into()]).unwrap();
        let a = storage
            .add_account(Network::Reddit, "alice", "Alice", "")
            .unwrap();
        let _ = a;
        for i in 0..3 {
            let mut p = crate::models::Post {
                id: 0,
                network: Network::Reddit,
                account_handle: "alice".into(),
                external_id: format!("p{i}"),
                author: "bob".into(),
                content: format!("rust post {i}\nsecond line"),
                url: format!("https://reddit.com/p{i}"),
                created_at: chrono::Utc::now() - chrono::Duration::hours(i as i64),
                fetched_at: chrono::Utc::now(),
                images: Vec::new(),
            };
            let _ = &mut p;
            storage.insert_post(&p, &[t]).unwrap();
        }
        app_with(storage)
    }

    #[test]
    fn new_app_starts_on_feed_and_loads_data() {
        let app = seeded_app();
        assert_eq!(app.screen, Screen::Feed);
        assert_eq!(app.topics.len(), 1);
        assert_eq!(app.accounts.len(), 1);
        assert_eq!(app.feed.len(), 3);

        // Empty storage leaves feed empty without error.
        let app = app_with(Storage::in_memory().unwrap());
        assert!(app.feed.is_empty());
    }

    #[test]
    fn screen_switching_and_quit() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('1')));
        assert_eq!(app.screen, Screen::Accounts);
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.screen, Screen::Topics);
        app.on_key(key(KeyCode::Char('3')));
        assert_eq!(app.screen, Screen::Feed);
        app.on_key(key(KeyCode::Char('4')));
        assert_eq!(app.screen, Screen::Sync);
        assert_eq!(app.sync_log, vec!["press s to sync".to_string()]);
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn topic_cycling_resets_feed_position() {
        let storage = Storage::in_memory().unwrap();
        let t1 = storage.add_topic("a", &["x".into()]).unwrap();
        let t2 = storage.add_topic("b", &["y".into()]).unwrap();
        let mut post = crate::models::Post {
            id: 0,
            network: Network::X,
            account_handle: "u".into(),
            external_id: "e1".into(),
            author: "u".into(),
            content: "x marks".into(),
            url: String::new(),
            created_at: chrono::Utc::now(),
            fetched_at: chrono::Utc::now(),
            images: Vec::new(),
        };
        storage.insert_post(&post, &[t1]).unwrap();
        post.external_id = "e2".into();
        post.content = "y town".into();
        storage.insert_post(&post, &[t2]).unwrap();

        let mut app = app_with(storage);
        assert_eq!(app.topics[0].name, "a");
        assert_eq!(app.feed.len(), 1);

        // 't' cycles to topic b and its feed.
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.topics[app.topic_sel].name, "b");
        assert_eq!(app.feed.len(), 1);
        assert!(app.feed[0].content.contains("y town"));
    }

    #[test]
    fn feed_pagination_clamps_at_zero() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.feed_offset, 10);
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(app.feed_offset, 0);
        // 'p' again saturates at zero.
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(app.feed_offset, 0);
    }

    #[test]
    fn selection_moves_within_bounds() {
        let mut app = seeded_app();
        // Feed: down wraps 0 -> 1 -> 2 -> 0.
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.feed_sel, 1);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.feed_sel, 2);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.feed_sel, 0);
        // Up from 0 wraps to len-1.
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.feed_sel, 2);

        // Accounts list.
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.list_sel, 0); // single item wraps
        // Sync screen has no list.
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.list_sel, 0);
    }

    #[test]
    fn detail_opens_on_enter_and_closes_on_q_esc() {
        let mut app = seeded_app();
        assert!(app.detail.is_none());
        app.on_key(key(KeyCode::Enter));
        assert!(app.detail.is_some());
        let d = app.detail.as_ref().unwrap();
        assert_eq!(d.post.external_id, "p0"); // feed_sel starts at newest
        assert!(d.previews.is_empty());

        // In detail mode, keys besides q/esc are ignored...
        app.on_key(key(KeyCode::Char('1')));
        assert!(app.detail.is_some());
        assert_eq!(app.screen, Screen::Feed);
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.detail.is_none());
    }

    #[test]
    fn account_input_parses_all_shapes() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('1'))); // accounts screen
        app.on_key(key(KeyCode::Char('a'))); // start add
        assert!(app.input.is_some());

        // Esc cancels.
        app.on_key(key(KeyCode::Esc));
        assert!(app.input.is_none());
        assert_eq!(app.accounts.len(), 1);

        // Basic NETWORK:HANDLE.
        app.on_key(key(KeyCode::Char('a')));
        for c in "reddit:bob".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.accounts.len(), 2);
        assert!(app.accounts.iter().any(|a| a.handle == "bob"));
        assert_eq!(app.message, "added");

        // DISPLAY and SECRET parts.
        app.on_key(key(KeyCode::Char('a')));
        for c in "x:carol:Carol C:envT".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        let carol = app.accounts.iter().find(|a| a.handle == "carol").unwrap();
        assert_eq!(carol.display_name, "Carol C");
        assert_eq!(carol.secret_ref, "envT");
        assert_eq!(carol.network, Network::X);

        // Bad network name reports an error, adds nothing.
        app.on_key(key(KeyCode::Char('a')));
        for c in "nope:dave".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(app.message.starts_with("error:"));
        assert!(!app.accounts.iter().any(|a| a.handle == "dave"));

        // Missing handle reports usage error.
        app.on_key(key(KeyCode::Char('a')));
        for c in "justanetwork".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(app.message.contains("expected NETWORK:HANDLE"), "{}", app.message);
    }

    #[test]
    fn backspace_edits_input_buffer() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('a')));
        for c in "reddit:bo".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Backspace));
        for c in "ob".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        assert!(app.accounts.iter().any(|a| a.handle == "bob"));
    }

    #[test]
    fn topic_input_adds_name_and_keywords() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('2')));
        app.on_key(key(KeyCode::Char('a')));
        for c in "news:rust, go, tui".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        let t = app.topics.iter().find(|t| t.name == "news").unwrap();
        assert_eq!(t.keywords, vec!["rust".to_string(), "go".to_string(), "tui".to_string()]);

        // Name-only topic.
        app.on_key(key(KeyCode::Char('a')));
        for c in "empty".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_key(key(KeyCode::Enter));
        let t = app.topics.iter().find(|t| t.name == "empty").unwrap();
        assert!(t.keywords.is_empty());

        // Empty name rejected. The colon form "":kw" trims to empty name.
        app.on_key(key(KeyCode::Char('a')));
        app.on_key(key(KeyCode::Char(':')));
        app.on_key(key(KeyCode::Char('k')));
        app.on_key(key(KeyCode::Enter));
        assert!(app.message.starts_with("error:"));

        // Empty input commit is a no-op without message change.
        let before = app.message.clone();
        app.on_key(key(KeyCode::Char('a')));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.message, before);
    }

    #[test]
    fn delete_selected_removes_account_and_topic() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('d')));
        assert_eq!(app.message, "deleted");
        assert!(app.accounts.is_empty());

        app.on_key(key(KeyCode::Char('2')));
        app.on_key(key(KeyCode::Char('d')));
        assert_eq!(app.message, "deleted");
        assert!(app.topics.is_empty());
        // Refreshing the feed on an empty topic list clears it.
        app.on_key(key(KeyCode::Char('3')));
        assert!(app.feed.is_empty());
    }

    #[test]
    fn toggle_selected_flips_enabled_state() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('2')));
        assert!(app.topics[0].enabled);
        app.on_key(key(KeyCode::Char('e')));
        assert_eq!(app.message, "toggled");
        assert!(!app.topics[0].enabled);
        app.on_key(key(KeyCode::Char('e')));
        assert!(app.topics[0].enabled);

        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('e')));
        assert!(!app.accounts[0].enabled);
    }

    #[test]
    fn toggle_on_empty_feed_reports_nothing_to_toggle() {
        let mut app = app_with(Storage::in_memory().unwrap());
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Char('e')));
        assert_eq!(app.message, "nothing to toggle");
    }

    #[test]
    fn sync_screen_run_collects_report_lines() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('s')));
        // The seeded reddit account has no reachable endpoint; sync reports
        // an error line but the log must have exactly one entry.
        assert_eq!(app.sync_log.len(), 1);
        assert!(app.sync_log[0].starts_with("reddit alice:") || app.sync_log[0].starts_with("sync failed"));
    }

    #[test]
    fn render_all_screens_and_overlays_on_test_backend() {
        // Accounts/topics/feed/sync + input popup + detail popup.
        let mut app = seeded_app();

        for screen_key in ['1', '2', '3', '4'] {
            app.on_key(key(KeyCode::Char(screen_key)));
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
        }

        // Input popup on accounts screen.
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('a')));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        // Detail overlay on feed screen.
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Enter));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
    }

    #[test]
    fn render_empty_states() {
        // No topics/accounts/feed at all: every screen renders without panic.
        let mut app = app_with(Storage::in_memory().unwrap());
        for screen_key in ['1', '2', '3', '4'] {
            app.on_key(key(KeyCode::Char(screen_key)));
            let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
            terminal.draw(|f| app.render(f)).unwrap();
        }
    }

    #[test]
    fn osc8_wraps_label_with_hyperlink_escape() {
        let s = osc8("https://x.com", "label");
        assert!(s.starts_with("\x1b]8;;https://x.com\x1b\\"));
        assert!(s.ends_with("\x1b]8;;\x1b\\"));
        assert!(s.contains("label"));
    }

    #[test]
    fn centered_rect_keeps_minimums_and_clamps() {
        let area = Rect::new(0, 0, 100, 50);
        let r = centered_rect(area, 70, 3);
        assert_eq!(r.width, 70);
        assert_eq!(r.height, 3);
        assert_eq!(r.x, 15);
        assert_eq!(r.y, 23);

        // Tiny area: width >= 20 clamped to area width; height >= 3.
        let tiny = Rect::new(0, 0, 10, 4);
        let r = centered_rect(tiny, 50, 50);
        assert_eq!(r.width, 10);
        assert_eq!(r.height, 3);

        // Zero area cannot exceed bounds.
        let zero = Rect::new(0, 0, 0, 0);
        let r = centered_rect(zero, 50, 50);
        assert_eq!((r.width, r.height), (0, 0));
    }

    #[test]
    fn input_screen_other_screens_commit_is_noop() {
        let mut app = seeded_app();
        app.on_key(key(KeyCode::Char('3'))); // feed screen
        app.input = Some(Input {
            prompt: "p".into(),
            buffer: "reddit:zz".into(),
        });
        app.on_key(key(KeyCode::Enter));
        assert!(app.input.is_none());
        // Feed screen has no add semantics; nothing added, no error.
        assert!(!app.accounts.iter().any(|a| a.handle == "zz"));
    }

    #[test]
    fn non_press_key_kinds_are_ignored_in_loop_helper() {
        // on_key itself doesn't check kind (the event loop does); ensure a
        // release-kind key still routes without panic here.
        let mut app = seeded_app();
        let k = event::KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: event::KeyModifiers::empty(),
            kind: event::KeyEventKind::Release,
            state: event::KeyEventState::empty(),
        };
        app.on_key(k);
        assert!(app.should_quit);
    }
}
