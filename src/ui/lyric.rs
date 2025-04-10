use std::time::Duration;

use crate::{
    client::get_lyric_client,
    error::LyricError,
    song::{
        LyricLine, LyricParser, PlayTime, PlayerAction, SongInfo, get_current_song,
        get_current_time_song, player_action,
    },
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Padding, Paragraph, Widget, Wrap},
};

use super::{LINE_STYLE, LINE_TARGET_STYLE, LYRIC_GAUGE_STYLE, LYRIC_HEADER_STYLE, render_error};

#[derive(Clone, Default)]
pub(super) struct LyricScreen {
    pub(super) state: LyricState,
}

impl LyricScreen {
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
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
            .style(LYRIC_HEADER_STYLE);

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
            LYRIC_GAUGE_STYLE,
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

            let style = if is_current {
                LINE_TARGET_STYLE
            } else {
                LINE_STYLE
            };

            let line = Line::styled(line_text, style);
            lines.push(line);
        }

        let block = Block::default()
            .title(self.get_window_title())
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1));

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    pub fn handle_key_event(&mut self, key_event: &KeyEvent) {
        match key_event.code {
            KeyCode::Char('d') | KeyCode::Delete => self.delete(),
            KeyCode::Left => self.state.action(PlayerAction::Left),
            KeyCode::Right => self.state.action(PlayerAction::Right),
            KeyCode::Char(' ') => self.state.action(PlayerAction::Toggle),
            KeyCode::Char('n') | KeyCode::Char('j') => self.state.action(PlayerAction::Next),
            KeyCode::Char('p') | KeyCode::Char('k') => self.state.action(PlayerAction::Previous),
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

// 新增显示参数结构体
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewMetrics {
    /// 可见行数
    pub visible_lines: usize,
    /// 总内容高度
    pub content_height: usize,
    /// 最大可滚动范围
    pub scroll_range: usize,
}

// 界面状态管理
#[derive(Clone, Default)]
pub struct LyricState {
    // 当前歌曲
    pub song: SongInfo,
    /// 播放时间
    pub play_time: PlayTime,
    /// 当前歌词
    pub lyrics: Vec<LyricLine>,
    /// 目标滚动位置
    pub target_scroll: usize,
    /// 新增显示参数
    pub view_metrics: ViewMetrics,
    /// 新增错误状态
    pub error_message: Option<String>,
    /// 重试计数器
    pub retry_counter: u32,
    /// 进度
    pub progress: f64,
}

impl LyricState {
    // 预计算显示参数
    pub fn calculate_metrics(&mut self, area: Size) {
        let content_height = self.lyrics.len();
        let viewport_height = area.height as usize;
        let visible_lines = viewport_height.saturating_sub(2); // 保留边界空间
        let scroll_range = content_height.saturating_sub(visible_lines);

        self.view_metrics = ViewMetrics {
            visible_lines,
            content_height,
            scroll_range,
        };
    }

    fn reset(&mut self) {
        *self = LyricState::default();
    }

    pub async fn update(&mut self) {
        self.error_message = None; // 清除旧错误        

        match self.try_update().await {
            Ok(_) => self.retry_counter = 0,
            Err(e) => {
                self.handle_error(e).await;
            }
        }
    }

    async fn try_update(&mut self) -> Result<(), LyricError> {
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
            let doc = get_lyric_client().get_lyric(&song).await?;
            self.lyrics = LyricParser::parse(&doc, song.duration)?;
        }

        // 获取当前播放进度
        self.play_time = get_current_time_song(self.play_time.clone())?;
        self.progress = self.play_time.current_time / song.duration;

        // 更新滚动位置
        if let Some(pos) = self.find_current_line() {
            let target_offset = pos.saturating_sub(self.view_metrics.visible_lines / 2);
            self.target_scroll = target_offset.min(self.view_metrics.scroll_range);
        }

        Ok(())
    }

    /// 当前播放的 line
    pub fn find_current_line(&self) -> Option<usize> {
        self.lyrics
            .iter()
            .enumerate()
            .find(|(_, line)| {
                self.play_time.current_time >= line.timestamp_start
                    && self.play_time.current_time < line.timestamp_end
            })
            .map(|(i, _)| i)
    }

    async fn handle_error(&mut self, error: LyricError) {
        self.retry_counter += 1;
        let error_msg = format!("Error: {} (Retry {}/5)", error, self.retry_counter);

        log::error!("{}", error_msg);
        self.error_message = Some(error_msg);

        if self.retry_counter < 5 {
            log::debug!("Retrying in 2 seconds...");
            tokio::time::sleep(Duration::from_secs(2)).await;
        } else {
            log::error!("Maximum retries reached");
            // self.error_message = Some("Maximum retries reached".into());
        }
    }

    pub fn delete(&self) {
        if !self.song.title.is_empty() {
            get_lyric_client().cache.delete(&self.song);
        }
    }

    pub fn action(&self, action: PlayerAction) {
        if let Err(e) = player_action(action, &self.song) {
            log::error!("Action: {e}");
        }
    }
}
