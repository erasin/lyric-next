use std::time::Duration;

use ratatui::layout::Size;

use crate::{
    client::get_lyric_client,
    error::LyricError,
    song::{
        LyricLine, LyricParser, PlayTime, PlayerAction, SongInfo, get_current_song,
        get_current_time_song, player_action,
    },
};

// 新增显示参数结构体
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewMetrics {
    /// 可见行数
    pub visible_lines: usize,
    /// 总内容高度
    pub content_height: usize,
    /// 最大可滚动范围
    pub scroll_range: usize,
    /// 视口高度
    pub viewport_height: usize,
    /// 单行高度
    pub line_height: u16,
}

// 界面状态管理
#[derive(Clone, Default)]
pub struct AppState {
    /// 是否通过
    pub valid: bool,
    // 当前歌曲
    pub song: SongInfo,
    /// 播放时间
    pub play_time: PlayTime,
    /// 当前歌词
    pub lyrics: Vec<LyricLine>,
    /// 目标滚动位置
    pub target_scroll: usize,
    /// 当前实际滚动位置
    pub current_scroll: f64,
    /// 新增显示参数
    pub view_metrics: ViewMetrics,
    /// 新增错误状态
    pub error_message: Option<String>,
    /// 重试计数器
    pub retry_counter: u32,
    /// 进度
    pub progress: u16,
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

    fn reset(&mut self) {
        if self.valid {
            *self = AppState::default();
        }
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
            self.valid = true;
            self.song = song.clone();
            let doc = get_lyric_client().get_lyric(&song).await?;
            self.lyrics = LyricParser::parse(&doc, song.duration)?;
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
        if self.valid {
            get_lyric_client().cache.delete(&self.song);
        }
    }

    pub fn action(&self, action: PlayerAction) {
        if let Err(e) = player_action(action, &self.song) {
            log::error!("Action: {e}");
        }
    }
}
