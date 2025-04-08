use std::time::Duration;

use crate::ui::LyricWidget;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
};
use tokio_stream::StreamExt;

#[derive(Clone, Default)]

pub struct App {
    exit: bool,

    lyric_widget: LyricWidget,
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
                    self.lyric_widget.update().await;
                    terminal.draw(|frame| self.draw(frame))?;
                },
                Some(Ok(event)) = events.next() => self.handle_event(&event),
            }
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        // 创建垂直布局
        let chunks = Layout::new(
            Direction::Vertical,
            [
                Constraint::Length(4), // 标题栏目
                Constraint::Length(1), // 进度
                Constraint::Min(1),    // 歌词区域
            ],
        );

        let [header_chunk, gauge_chunk, lyric_chunk] = chunks.areas(frame.area());

        let size = lyric_chunk.as_size();
        self.lyric_widget.update_size(size);

        let buf = frame.buffer_mut();
        self.lyric_widget.render_title(header_chunk, buf);
        self.lyric_widget.render_gauge(gauge_chunk, buf);
        self.lyric_widget.lyric_render(lyric_chunk, buf);
    }

    fn handle_event(&mut self, event: &Event) {
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                self.handle_key_event(key);
            }
        }
    }

    fn handle_key_event(&mut self, key_event: &KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Char('d') => self.delete(),
            KeyCode::Esc => self.exit(),
            _ => {}
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }

    fn delete(&mut self) {
        self.lyric_widget.state.delete();
    }
}
