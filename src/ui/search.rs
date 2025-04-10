use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    symbols,
    text::Line,
    widgets::{
        Block, Borders, HighlightSpacing, List, ListItem, ListState, Paragraph, StatefulWidget,
        Widget,
    },
};

use crate::{
    error::LyricError,
    song::{SongInfo, get_current_song},
};

use super::*;

// search
#[derive(Clone, Default)]
pub(super) struct SearchScreen {
    state: SearchState,
    list_state: ListState,
}

impl SearchScreen {
    // 主渲染函数
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let _ = self.state.update();

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

    pub fn handle_key_event(&mut self, key_event: &KeyEvent) {
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
            .state
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

#[derive(Clone, Default)]
pub struct SearchState {
    song: SongInfo,
    // current_page: usize,
    // total_pages: usize,
    list: Vec<String>,
}

impl SearchState {
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
}
