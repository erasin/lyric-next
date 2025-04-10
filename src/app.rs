use std::{borrow::Cow, time::Duration};

use crate::{
    error::LyricError,
    song::{PlayerAction, SongInfo, get_current_song},
    state::LyricState,
};
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect, Size},
    style::{
        Color, Modifier, Style, Stylize,
        palette::{
            material::{BLUE, GREEN},
            tailwind::SLATE,
        },
    },
    symbols,
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Gauge, HighlightSpacing, List, ListItem, ListState, Padding,
        Paragraph, StatefulWidget, Widget, Wrap,
    },
};
use tokio_stream::StreamExt;

#[derive(Default, Clone, Debug)]
enum Screen {
    #[default]
    Lyric,
    Search,
    Help,
}

#[derive(Clone, Default)]
pub struct App {
    exit: bool,
    screen: Screen,

    lyric: LyricScreen,
    search: SearchScreen,
    help: HelpScreen,
}

impl App {
    const FRAMES_PER_SECOND: f32 = 30.0;

    // 保持UI和主循环不变
    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let period = Duration::from_secs_f32(1.0 / Self::FRAMES_PER_SECOND);
        let mut interval = tokio::time::interval(period);
        let mut events = EventStream::new();

        while !self.exit {
            tokio::select! {
                _ = interval.tick() => {
                    self.lyric.update().await;
                    terminal.draw(|frame| self.draw(frame))?;
                },
                Some(Ok(event)) = events.next() => self.handle_event(&event),
            }
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let buf = frame.buffer_mut();
        match self.screen {
            Screen::Lyric => self.lyric.render(area, buf),
            Screen::Search => self.search.render(area, buf),
            Screen::Help => self.help.render(area, buf),
        }
    }

    fn handle_event(&mut self, event: &Event) {
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                match self.screen {
                    Screen::Lyric => match key.code {
                        KeyCode::Char('h') | KeyCode::Char('?') => self.screen = Screen::Help,
                        KeyCode::Char('s') => self.screen = Screen::Search,
                        KeyCode::Char('q') | KeyCode::Esc => self.exit(),
                        _ => self.lyric.handle_key_event(key),
                    },
                    Screen::Search => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => self.screen = Screen::Lyric,
                        KeyCode::Char('h') | KeyCode::Char('?') => self.screen = Screen::Help,
                        _ => self.search.handle_key_event(key),
                    },
                    Screen::Help => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => self.screen = Screen::Lyric,
                        _ => {}
                    },
                }
            }
        }
    }

    /// 关闭
    fn exit(&mut self) {
        self.exit = true;
    }
}

#[derive(Clone, Default)]
struct LyricScreen {
    pub(super) state: LyricState,
}

impl LyricScreen {
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        // 创建垂直布局
        let [header_chunk, lyric_chunk, gauge_chunk] = Layout::new(
            Direction::Vertical,
            [
                Constraint::Length(4), // 标题栏目
                Constraint::Min(1),    // 歌词区域
                Constraint::Length(1), // 进度
            ],
        )
        .areas(area);

        let size = lyric_chunk.as_size();
        self.update_size(size);

        self.render_title(header_chunk, buf);
        self.render_lyric(lyric_chunk, buf);
        self.render_gauge(gauge_chunk, buf);
    }

    fn get_window_title(&self) -> String {
        match !self.state.song.title.is_empty() {
            true => self.state.song.title.clone(),
            false => " No song playing ".into(),
        }
    }

    pub fn render_title(&self, area: Rect, buf: &mut Buffer) {
        if self.state.song.title.is_empty() {
            return;
        }
        // 渲染标题区块
        let header_block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::LightBlue));

        // 显示歌曲信息
        let song = &self.state.song.clone();

        let line_title = song.title.clone();
        let line_artist = song.artist.clone();

        let lines = vec![Line::raw(line_title), Line::raw(line_artist)];

        Paragraph::new(lines)
            .block(header_block)
            .centered()
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    /// 进度
    pub fn render_gauge(&self, area: Rect, buf: &mut Buffer) {
        if self.state.song.title.is_empty() {
            return;
        }

        let song = &self.state.song.clone();

        let label = Span::styled(
            format!(
                "{:0>2}:{:0>2} / {:0>2}:{:0>2}",
                (&self.state.play_time.current_time / 60.0).floor() as u64,
                (&self.state.play_time.current_time % 60.0).floor() as u64,
                (song.duration / 60.0).floor() as u64,
                (song.duration % 60.0).floor() as u64,
            ),
            Style::new().italic().bold().fg(Color::White),
        );

        Gauge::default()
            .gauge_style(Style::new().blue().on_dark_gray())
            .percent((self.state.progress * 100.0) as u16)
            .label(label)
            .render(area, buf);
    }

    /// 渲染歌词
    pub fn render_lyric(&self, area: Rect, buf: &mut Buffer) {
        let state = &self.state;

        // 渲染错误信息
        if let Some(err_msg) = &state.error_message {
            render_error(area, buf, err_msg);
            return;
        }

        // 使用预计算的显示参数
        let metrics = &state.view_metrics;
        let start = state.target_scroll.min(metrics.scroll_range);
        let end = (start + metrics.visible_lines).min(metrics.content_height);
        let mut lines = Vec::new();
        for (i, line) in state.lyrics[start..end].iter().enumerate() {
            let is_current = start + i == state.find_current_line().unwrap_or(0);

            #[cfg(debug_assertions)]
            let line_text = format!(
                "[{:0>2}:{:0>2}] {}",
                (line.timestamp_start / 60.0).floor() as u64,
                (line.timestamp_start % 60.0).floor() as u64,
                line.text
            );

            #[cfg(not(debug_assertions))]
            let line_text = format!("{}", line.text);

            let style = Style::default()
                .fg(if is_current {
                    Color::Yellow
                } else {
                    Color::Gray
                })
                .add_modifier(if is_current {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });

            let line = Line::styled(line_text, style);
            lines.push(line);
        }

        let block = Block::default()
            .title(self.get_window_title())
            .borders(Borders::ALL);

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    fn handle_key_event(&mut self, key_event: &KeyEvent) {
        match key_event.code {
            KeyCode::Char('d') | KeyCode::Delete => self.delete(),
            KeyCode::Left => self.state.action(PlayerAction::Left),
            KeyCode::Right => self.state.action(PlayerAction::Right),
            KeyCode::Char(' ') => self.state.action(PlayerAction::Toggle),
            KeyCode::Char('n') => self.state.action(PlayerAction::Next),
            KeyCode::Char('p') => self.state.action(PlayerAction::Previous),
            _ => {}
        }
    }

    /// 状态刷新
    pub async fn update(&mut self) {
        self.state.update().await;
    }

    /// 尺寸变动
    pub fn update_size(&mut self, size: Size) {
        self.state.calculate_metrics(size);
    }

    /// 删除
    fn delete(&mut self) {
        self.state.delete();
    }
}

// search
#[derive(Clone, Default)]
struct SearchScreen {
    song: SongInfo,
    current_page: usize,
    total_pages: usize,
    selected: usize,
    list: Vec<String>,
    list_state: ListState,
}

impl SearchScreen {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn update(&mut self) -> Result<(), LyricError> {
        // 获取当前播放器和歌曲信息
        let song = match get_current_song() {
            Ok(s) => s,
            Err(LyricError::NoPlayerFound) => {
                self.reset();
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // 歌曲发生变化时重新加载歌词
        if song != self.song {
            self.reset();
            self.song = song.clone();
            self.list = vec![
                "test1".to_string(),
                "test2".to_string(),
                "test3".to_string(),
                "test4".to_string(),
                "test5".to_string(),
            ];
        }

        Ok(())
    }

    // 主渲染函数
    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let _ = self.update();

        // 整体垂直布局
        let [header_chunk, list_chunk, footer_chunk] = Layout::new(
            Direction::Vertical,
            [
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
            ],
        )
        .areas(area);

        self.render_header(header_chunk, buf);
        self.render_list(list_chunk, buf);
        self.render_footer(footer_chunk, buf);
    }

    fn handle_key_event(&mut self, key_event: &KeyEvent) {
        match key_event.code {
            KeyCode::Char('l') | KeyCode::Enter => self.download(),
            KeyCode::Up | KeyCode::Char('p') | KeyCode::Char('k') => self.selected_up(),
            KeyCode::Down | KeyCode::Char('n') | KeyCode::Char('j') => self.selected_down(),
            _ => {}
        }
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("Lyric 列表")
            .bold()
            .centered()
            .render(area, buf);
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("使用 ↓↑ or jk 选择, l or enter 下载")
            .centered()
            .render(area, buf);
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer) {
        let block = Block::new()
            .title(Line::raw("搜索").centered())
            .borders(Borders::TOP)
            .border_set(symbols::border::EMPTY)
            .border_style(TODO_HEADER_STYLE)
            .bg(NORMAL_ROW_BG);

        // Iterate through all elements in the `items` and stylize them.
        let items: Vec<ListItem> = self
            .list
            .iter()
            .enumerate()
            .map(|(i, todo_item)| {
                let color = alternate_colors(i);
                Line::raw(todo_item).bg(color).into()
            })
            .collect();

        // Create a List from all list items and highlight the currently selected one
        let list = List::new(items)
            .block(block)
            .highlight_style(SELECTED_STYLE)
            .highlight_symbol(">")
            .highlight_spacing(HighlightSpacing::Always);

        StatefulWidget::render(list, area, buf, &mut self.list_state);
    }

    fn selected_up(&mut self) {
        self.list_state.select_previous();
    }

    fn selected_down(&mut self) {
        self.list_state.select_next();
    }

    fn download(&self) {}
}

const TODO_HEADER_STYLE: Style = Style::new().fg(SLATE.c100).bg(BLUE.c800);
const NORMAL_ROW_BG: Color = SLATE.c950;
const ALT_ROW_BG_COLOR: Color = SLATE.c900;
const SELECTED_STYLE: Style = Style::new().bg(SLATE.c800).add_modifier(Modifier::BOLD);
const TEXT_FG_COLOR: Color = SLATE.c200;
const COMPLETED_TEXT_FG_COLOR: Color = GREEN.c500;

const fn alternate_colors(i: usize) -> Color {
    if i % 2 == 0 {
        NORMAL_ROW_BG
    } else {
        ALT_ROW_BG_COLOR
    }
}

#[derive(Clone, Default)]
struct HelpScreen;

impl HelpScreen {
    // 帮助
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::new(
            Direction::Horizontal,
            [Constraint::Min(1), Constraint::Min(1), Constraint::Min(1)],
        );
        let [lyric_chunk, search_chunk, help_chunk] = chunks.areas(area);

        let lines = vec![
            ("    h | ? ", " 帮助."),
            ("  q | ESC ", " 退出."),
            ("d | Delete ", " 删除当前歌词"),
            ("      Left ", " 快退"),
            ("     Right ", "快进"),
            ("         n ", "下一曲"),
            ("         p ", "上一曲"),
        ];
        help(lines).render(lyric_chunk, buf);

        // search
        let lines = vec![("q | ESC ", " 退出到歌词界面.")];
        help(lines).render(search_chunk, buf);

        // help
        let lines = vec![
            ("q | ESC ", " 退出到歌词界面."),
            ("h | ?   ", " 帮助."),
            ("n | Down", "下一个"),
            ("p | Up  ", "上一个"),
            ("l | Enter ", "上一个"),
        ];
        help(lines).render(help_chunk, buf);
    }
}

// 提取的创建行函数
fn help<'a>(lines: Vec<(&'a str, &'a str)>) -> Paragraph<'a> {
    let lines: Vec<Line> = lines
        .into_iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(key, Style::default().fg(Color::Blue)),
                Span::raw(":"),
                Span::raw(description),
            ])
        })
        .collect();

    Paragraph::new(lines)
        .block(Block::default().title("搜索").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

pub fn render_error(area: Rect, buf: &mut Buffer, err_msg: &str) {
    Paragraph::new(err_msg)
        .style(Style::default().fg(Color::Red))
        .block(
            Block::default()
                .title("ERROR")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL),
        )
        .render(area, buf);
}
