use std::time::Duration;

use crate::{song::PlayerAction, state::AppState};
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Widget, Wrap},
};
use tokio_stream::StreamExt;

#[derive(Clone, Default)]

pub struct App {
    exit: bool,
    help: bool,

    pub state: AppState,
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
                    self.state.update().await;
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

        // 创建垂直布局
        let chunks = Layout::new(
            Direction::Vertical,
            [
                Constraint::Length(4), // 标题栏目
                Constraint::Min(1),    // 歌词区域
                Constraint::Length(1), // 进度
            ],
        );

        let [header_chunk, lyric_chunk, gauge_chunk] = chunks.areas(area);

        let size = lyric_chunk.as_size();
        self.update_size(size);

        self.render_title(header_chunk, buf);
        self.render_lyric(lyric_chunk, buf);
        self.render_gauge(gauge_chunk, buf);

        if self.help {
            Clear.render(area, buf);
            self.render_helper(area, buf);
        }
    }

    fn handle_event(&mut self, event: &Event) {
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                self.handle_key_event(key);
            }
        }
    }

    fn handle_key_event(&mut self, key_event: &KeyEvent) {
        if self.help {
            match key_event.code {
                KeyCode::Char('q') | KeyCode::Esc => self.help = false,
                _ => {}
            }
            return;
        }

        match key_event.code {
            KeyCode::Char('h') | KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('q') | KeyCode::Esc => self.exit(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete(),
            KeyCode::Left => self.state.action(PlayerAction::Left),
            KeyCode::Right => self.state.action(PlayerAction::Right),
            KeyCode::Char(' ') => self.state.action(PlayerAction::Toggle),
            KeyCode::Char('n') => self.state.action(PlayerAction::Next),
            KeyCode::Char('p') => self.state.action(PlayerAction::Previous),
            _ => {}
        }
    }

    /// 关闭
    fn exit(&mut self) {
        self.exit = true;
    }

    /// 删除
    fn delete(&mut self) {
        self.state.delete();
    }

    /// 状态刷新
    pub async fn update(&mut self) {
        self.state.update().await;
    }

    /// 尺寸变动
    pub fn update_size(&mut self, size: Size) {
        self.state.calculate_metrics(size);
    }

    fn get_window_title(&self) -> String {
        match &self.state.valid {
            true => self.state.song.title.clone(),
            false => " No song playing ".into(),
        }
    }

    pub fn render_title(&self, area: Rect, buf: &mut Buffer) {
        if !self.state.valid {
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
        if !self.state.valid {
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
            let error_block = Paragraph::new(err_msg.clone())
                .style(Style::default().fg(Color::Red))
                .block(
                    Block::default()
                        .title("ERROR")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL),
                );
            error_block.render(area, buf);
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

    // 帮助
    pub fn render_helper(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().title("HELP").borders(Borders::ALL);

        let lines = vec![
            Line::raw("h | ?   : 帮助."),
            Line::raw("q | ESC : 退出."),
            Line::raw("d | Delete  : 删除当前歌词"),
            Line::raw("Left : 快退"),
            Line::raw("Right: 快进"),
            Line::raw("n : 下一曲"),
            Line::raw("p : 上一曲"),
        ];

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }
}
