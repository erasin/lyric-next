use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use help::HelpScreen;
use lyric::LyricScreen;
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{
        Color, Modifier, Style,
        palette::material::{BLUE, GRAY, LIGHT_BLUE, WHITE, YELLOW},
    },
    widgets::{Block, Borders, Paragraph, Widget},
};
use search::SearchScreen;
use tokio_stream::StreamExt;

mod help;
mod lyric;
mod search;

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

const LINE_STYLE: Style = Style::new().fg(GRAY.c200);
const LINE_TARGET_STYLE: Style = Style::new().fg(YELLOW.c800).add_modifier(Modifier::BOLD);
const LYRIC_HEADER_STYLE: Style = Style::new().fg(BLUE.c400);
const LYRIC_GAUGE_STYLE: Style = Style::new()
    .add_modifier(Modifier::ITALIC)
    .add_modifier(Modifier::BOLD)
    .fg(WHITE);

const HELP_KEY_STYLE: Style = Style::new()
    .fg(LIGHT_BLUE.c400)
    .add_modifier(Modifier::BOLD);

const TODO_HEADER_STYLE: Style = Style::new().fg(GRAY.c100).bg(BLUE.c800);
const NORMAL_ROW_BG: Color = GRAY.c900;
const ALT_ROW_BG_COLOR: Color = GRAY.c900;
const SELECTED_STYLE: Style = Style::new().bg(GRAY.c800).add_modifier(Modifier::BOLD);

const fn alternate_colors(i: usize) -> Color {
    if i % 2 == 0 {
        NORMAL_ROW_BG
    } else {
        ALT_ROW_BG_COLOR
    }
}

fn render_error(area: Rect, buf: &mut Buffer, err_msg: &str) {
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
