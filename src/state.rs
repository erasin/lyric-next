use std::time::Duration;

use ratatui::layout::Size;

use crate::{
    client::get_lyric_client,
    error::LyricError,
    song::{LyricLine, LyricParser, PlayTime, SongInfo, get_current_song, get_current_time_song},
};

// 新增显示参数结构体
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewMetrics {
    pub visible_lines: usize,   // 可见行数
    pub content_height: usize,  // 总内容高度
    pub scroll_range: usize,    // 最大可滚动范围
    pub viewport_height: usize, // 视口高度
    pub line_height: u16,       // 单行高度
}

// 界面状态管理
#[derive(Clone, Default)]
pub struct AppState {
    pub current_song: Option<SongInfo>,
    pub play_time: PlayTime,
    pub lyrics: Vec<LyricLine>,
    pub target_scroll: usize,          // 目标滚动位置
    pub current_scroll: f64,           // 当前实际滚动位置
    pub view_metrics: ViewMetrics,     // 新增显示参数
    pub error_message: Option<String>, // 新增错误状态
    pub retry_counter: u32,            // 重试计数器
    pub progress: u16,                 // 进度
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
            viewport_height,
            line_height: 1, // 假设单行高度为1
        };
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
                // self.player = None.into();
                self.current_song = None;
                self.lyrics.clear();
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        // 歌曲发生变化时重新加载歌词
        if Some(song.clone()) != self.current_song {
            self.handle_song_change(&song).await?;
        }

        // 获取当前播放进度
        self.play_time = get_current_time_song(self.play_time.clone())?;

        self.progress = (self.play_time.current_time * 100.0 / song.duration) as u16;

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

    async fn handle_song_change(&mut self, song: &SongInfo) -> Result<(), LyricError> {
        self.current_song = Some(song.clone());
        self.lyrics.clear();
        self.target_scroll = 0;
        self.current_scroll = 0.0;

        if let Some(song) = &self.current_song {
            let doc = get_lyric_client().get_lyric(song).await?;
            self.lyrics = LyricParser::parse(&doc, song.duration)?;
        }

        Ok(())
    }

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
        if let Some(song) = &self.current_song {
            get_lyric_client().cache.delete(song);
        }
    }
}
