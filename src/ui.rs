use ratatui::{
    buffer::Buffer,
    layout::{Rect, Size},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Padding, Paragraph, Widget, Wrap},
};

use crate::state::AppState;

// 界面渲染
#[derive(Clone, Default)]
pub struct LyricWidget {
    pub state: AppState,
}

impl LyricWidget {
    pub async fn update(&mut self) {
        self.state.update().await;
    }

    pub fn update_size(&mut self, size: Size) {
        self.state.calculate_metrics(size);
    }

    fn get_window_title(&self) -> String {
        match &self.state.current_song {
            Some(song) => format!(" {} ", song.title),
            None => " No song playing ".into(),
        }
    }

    pub fn title_render(&self, area: Rect, buf: &mut Buffer) {
        if self.state.current_song.is_none() {
            return;
        }
        // 渲染标题区块
        let header_block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::LightBlue));

        // 显示歌曲信息
        let song = &self.state.current_song.clone().unwrap();

        let line_title = format!("{}", song.title);
        let line_artist = format!("{}", song.artist);

        // buf.set_string(Style::default().fg(Color::LightBlue));

        let lines = vec![Line::raw(line_title), Line::raw(line_artist)];

        Paragraph::new(lines)
            .block(header_block)
            .centered()
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    /// 进度
    pub fn gauge_render(&self, area: Rect, buf: &mut Buffer) {
        if self.state.current_song.is_none() {
            return;
        }

        let song = &self.state.current_song.clone().unwrap();

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
            .percent(self.state.progress)
            .label(label)
            .render(area, buf);
    }

    // 搜索
    // pub fn search_render(&self, area: Rect, buf: &mut Buffer) {}

    pub fn lyric_render(&self, area: Rect, buf: &mut Buffer) {
        let state = &self.state;

        // 渲染错误信息
        if let Some(err_msg) = &state.error_message {
            let error_block = Paragraph::new(err_msg.clone())
                .style(Style::default().fg(Color::Red))
                .block(Block::default().borders(Borders::ALL));
            error_block.render(area, buf);
            return;
        }

        // 使用预计算的显示参数
        let metrics = &state.view_metrics;
        let scroll_pos = state.current_scroll as usize;
        let start = scroll_pos.min(metrics.scroll_range);
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
}
