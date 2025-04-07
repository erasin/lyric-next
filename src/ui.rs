use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::{Rect, Size},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    client::get_lyric_client,
    error::LyricError,
    song::{LyricLine, LyricParser, PlayTime, SongInfo, get_current_song, get_current_time_song},
};

// 新增显示参数结构体
#[derive(Debug, Clone, Copy, Default)]
struct ViewMetrics {
    visible_lines: usize,   // 可见行数
    content_height: usize,  // 总内容高度
    scroll_range: usize,    // 最大可滚动范围
    viewport_height: usize, // 视口高度
    line_height: u16,       // 单行高度
}

// 界面状态管理
#[derive(Clone, Default)]
pub struct AppState {
    current_song: Option<SongInfo>,
    play_time: PlayTime,
    lyrics: Vec<LyricLine>,
    target_scroll: usize,          // 目标滚动位置
    current_scroll: f64,           // 当前实际滚动位置
    view_metrics: ViewMetrics,     // 新增显示参数
    error_message: Option<String>, // 新增错误状态
    retry_counter: u32,            // 重试计数器
}

impl AppState {
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
            viewport_height: viewport_height as usize,
            line_height: 1, // 假设单行高度为1
        };
    }

    async fn update(&mut self) {
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
        let result = get_current_song();
        let new_song = match result {
            Ok(s) => s,
            Err(LyricError::NoPlayerFound) => {
                // self.player = None.into();
                self.current_song = None;
                self.lyrics.clear();
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // 歌曲发生变化时重新加载歌词
        if Some(new_song.clone()) != self.current_song {
            self.handle_song_change(new_song).await?;
        }

        // 获取当前播放进度
        self.play_time = get_current_time_song(self.play_time.clone())?;

        // 更新滚动位置
        if let Some(pos) = self.find_current_line() {
            let target_offset = pos.saturating_sub(self.view_metrics.visible_lines / 2);
            self.target_scroll = target_offset.min(self.view_metrics.scroll_range);
        }

        // 平滑滚动插值（每秒移动20像素）
        let scroll_delta = (self.target_scroll as f64 - self.current_scroll) * 0.2;
        self.current_scroll += scroll_delta;

        Ok(())
    }

    async fn handle_song_change(&mut self, song: SongInfo) -> Result<(), LyricError> {
        self.current_song = Some(song);
        self.lyrics.clear();
        self.target_scroll = 0;
        self.current_scroll = 0.0;

        if let Some(song) = &self.current_song {
            let doc = get_lyric_client().get_lyric(song).await?;
            self.lyrics = LyricParser::parse(&doc, song.duration)?;
        }

        Ok(())
    }

    fn find_current_line(&self) -> Option<usize> {
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
        if let Some(song) = &self.current_song {
            get_lyric_client().cache.delete(song);
        }
    }
}

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
            Some(song) => format!(" Now Playing: {} ", song.title),
            None => " No song playing ".into(),
        }
    }
}

impl Widget for &LyricWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = &self.state;

        let block = Block::default()
            .title(self.get_window_title())
            .borders(Borders::ALL);
        block.render(area, buf);

        // 渲染错误信息
        if let Some(err_msg) = &state.error_message {
            let error_block = Paragraph::new(err_msg.clone())
                .style(Style::default().fg(Color::Red))
                .block(Block::default().borders(Borders::ALL));
            error_block.render(area, buf);
            return;
        }

        // 显示歌曲信息
        if let Some(song) = &state.current_song {
            let info_line = format!(
                " ♫ {} - {} {:0>2}:{:0>2} / {:0>2}:{:0>2}
                ",
                song.artist,
                song.title,
                (&state.play_time.current_time / 60.0).floor() as u64,
                (&state.play_time.current_time % 60.0).floor() as u64,
                (song.duration / 60.0).floor() as u64,
                (song.duration % 60.0).floor() as u64,
            );
            buf.set_string(
                area.x + 1,
                area.y,
                &info_line,
                Style::default().fg(Color::LightBlue),
            );
        }

        // 使用预计算的显示参数
        let metrics = &state.view_metrics;
        let scroll_pos = state.current_scroll as usize;
        let start = scroll_pos.min(metrics.scroll_range);
        let end = (start + metrics.visible_lines).min(metrics.content_height);
        for (i, line) in state.lyrics[start..end].iter().enumerate() {
            let y = area.y + i as u16;
            let is_current = start + i == state.find_current_line().unwrap_or(0);

            // 居中计算
            #[cfg(debug_assertions)]
            let line_text = format!(
                "[{:0>2}:{:0>2}] {}",
                (line.timestamp_start / 60.0).floor() as u64,
                (line.timestamp_start % 60.0).floor() as u64,
                line.text
            );

            #[cfg(not(debug_assertions))]
            let line_text = format!("{}", line.text);

            let text_width = line_text.chars().count() as u16;
            let x = area.x + (area.width - text_width) / 2;

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

            buf.set_string(x, y, &line_text, style);
        }
    }
}
