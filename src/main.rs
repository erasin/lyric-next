use anyhow::Result;
use async_trait::async_trait;
use base64::prelude::*;
use chrono::Local;
use clap::Parser;
// use color_eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use dirs::home_dir;
use mpris::{Player, PlayerFinder};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Widget},
};
use ropey::Rope;
use sanitize_filename::sanitize;
use serde_json::Value;
use std::io::Write;
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio_stream::StreamExt;

#[derive(Error, Debug)]
pub enum LyricError {
    #[error("MPRIS error: {0}")]
    MprisError(#[from] mpris::DBusError),

    #[error("MPRIS Find error: {0}")]
    MprisFindError(#[from] mpris::FindingError),

    #[error("HTTP error: {0}")]
    ReqwestError(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("base64 error: {0}")]
    DecodeError(#[from] base64::DecodeError),

    #[error("No active media player found")]
    NoPlayerFound,

    #[error("Failed to get cache path")]
    CachePathError,

    #[error("No lyrics found")]
    NoLyricFound,

    #[error("JSON parse error")]
    JsonError,

    #[error("Lyric validation failed")]
    LyricValidationFailed,

    #[error("Lyric decode failed")]
    LyricDecodeError,

    #[error("Invalid time format: {0}")]
    InvalidTimeFormat(String),

    #[error("Empty lyric content")]
    EmptyLyric,
}

const CACHE_DIR: &str = ".local/share/lyrics";

#[derive(Parser, Debug)]
#[clap(version, about)]
struct Args;

#[async_trait]
trait LyricFetcher: Send + Sync {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError>;
    fn source_name(&self) -> &'static str;
}

// 公共基础结构
struct BaseFetcher {
    client: reqwest::Client,
    retries: u8,
}

impl Default for BaseFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl BaseFetcher {
    fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            retries: 3,
        }
    }

    // 添加重试机制
    async fn fetch_with_retry(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<Value, LyricError> {
        let mut attempt = 0;
        loop {
            let response = request.try_clone().unwrap().send().await;
            match response {
                Ok(res) => return Ok(res.json().await?),
                Err(_e) if attempt < self.retries => {
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

// 网易云音乐实现
#[derive(Default)]
struct NeteaseFetcher {
    base: BaseFetcher,
}

#[async_trait]
impl LyricFetcher for NeteaseFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        log::debug!("Get song: {:?}", song);
        let search_url = "https://music.163.com/api/search/get/";

        let request = self.base.client.get(search_url).query(&[
            ("s", format!("{} {}", song.title, song.artist)),
            ("type", "1".into()),
            ("limit", "1".into()),
        ]);

        let json = self.base.fetch_with_retry(request).await?;
        log::debug!("Get song: {:?}", json);
        let song_id = json["result"]["songs"][0]["id"]
            .as_u64()
            .ok_or(LyricError::NoLyricFound)?;

        let lyric_url = format!("https://music.163.com/api/song/lyric?id={}&lv=1", song_id);
        let response = self.base.client.get(lyric_url).send().await?;

        let json: Value = response.json().await?;
        log::debug!("Get lyric: {:?}", json);
        json["lrc"]["lyric"]
            .as_str()
            .filter(|&s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or(LyricError::NoLyricFound)
    }

    fn source_name(&self) -> &'static str {
        "Netease"
    }
}

// QQ音乐实现
#[derive(Default)]
struct QQMusicFetcher {
    base: BaseFetcher,
}

#[async_trait]
impl LyricFetcher for QQMusicFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        // 1. 搜索歌曲
        let search_url = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";
        let response = self
            .base
            .client
            .get(search_url)
            .query(&[
                ("w", format!("{} {}", song.title, song.artist).as_str()),
                ("format", "json"),
                ("n", "1"),
                ("cr", "1"),
                ("g_tk", "5381"),
            ])
            .header("Referer", "https://y.qq.com/")
            .header("Host", "c.y.qq.com")
            .send()
            .await?;

        let json: Value = response.json().await?;
        let song_list = json["data"]["song"]["list"]
            .as_array()
            .ok_or(LyricError::NoLyricFound)?;

        let song_mid = song_list[0]["songmid"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;

        // 2. 获取歌词
        let lyric_url = "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric.fcg";
        let response = self
            .base
            .client
            .get(lyric_url)
            .query(&[("songmid", song_mid), ("format", "json"), ("g_tk", "5381")])
            .header("Referer", "https://y.qq.com/")
            .header("Host", "c.y.qq.com")
            .send()
            .await?;

        let json: Value = response.json().await?;
        let lyric = json["lyric"].as_str().ok_or(LyricError::NoLyricFound)?;

        // 处理Base64解码
        let decoded = BASE64_STANDARD
            .decode(lyric)
            .map_err(|_| LyricError::LyricDecodeError)?;

        let re = String::from_utf8(decoded).map_err(|_| LyricError::LyricDecodeError)?;
        if re.is_empty() {
            return Err(LyricError::NoLyricFound);
        }
        Ok(re)
    }

    fn source_name(&self) -> &'static str {
        "QQMusic"
    }
}

// Kugou音乐实现
#[derive(Default)]
struct KugouFetcher {
    base: BaseFetcher,
}

impl KugouFetcher {
    // 酷狗歌词解密函数
    fn decode_lyric(&self, encrypted: &str) -> Result<String, LyricError> {
        let bytes = BASE64_STANDARD.decode(encrypted)?;
        let key = b"kg@lrc$okm0qaz";
        let decrypted: Vec<u8> = bytes
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ key[i % key.len()])
            .collect();
        let re = String::from_utf8(decrypted).map_err(|_| LyricError::LyricDecodeError)?;
        if re.is_empty() {
            return Err(LyricError::NoLyricFound);
        }
        Ok(re)
    }
}

#[async_trait]
impl LyricFetcher for KugouFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        // 1. 搜索歌曲
        let search_url = "http://mobilecdn.kugou.com/api/v3/search/song";
        let response = self
            .base
            .client
            .get(search_url)
            .query(&[
                (
                    "keyword",
                    format!("{} {}", song.title, song.artist).as_str(),
                ),
                ("page", "1"),
                ("pagesize", "1"),
            ])
            .send()
            .await?;

        let json: Value = response.json().await?;
        let songs = json["data"]["info"]
            .as_array()
            .ok_or(LyricError::NoLyricFound)?;

        let song_hash = songs[0]["hash"].as_str().ok_or(LyricError::NoLyricFound)?;
        let album_id = songs[0]["album_id"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;

        let current_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // 2. 获取歌词
        let lyric_url = "http://krcs.kugou.com/search";
        let response = self
            .base
            .client
            .get(lyric_url)
            .query(&[
                (
                    "keyword",
                    format!("{} {}", song.title, song.artist).as_str(),
                ),
                ("hash", song_hash),
                ("album_id", album_id),
                ("_", &current_timestamp.to_string()),
            ])
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await?;

        let json: Value = response.json().await?;
        let lyric = json["content"].as_str().ok_or(LyricError::NoLyricFound)?;

        // 处理酷狗特有的加密歌词
        let decoded = self.decode_lyric(lyric)?;
        Ok(decoded)
    }

    fn source_name(&self) -> &'static str {
        "Kugou"
    }
}

// Spotify音乐实现
#[derive(Default)]
struct SpotifyFetcher {
    base: BaseFetcher,
}

#[async_trait]
impl LyricFetcher for SpotifyFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        // 假设使用的第三方Spotify歌词API如下（实际应使用真实的API）
        let search_url = "https://api.thirdparty.com/spotify/lyrics";
        let response = self
            .base
            .client
            .get(search_url)
            .query(&[("track", &song.title), ("artist", &song.artist)])
            .send()
            .await?;

        let json: Value = response.json().await?;
        let lyric = json["lyrics"].as_str().ok_or(LyricError::NoLyricFound)?;

        if lyric.is_empty() {
            return Err(LyricError::NoLyricFound);
        }

        // 假设第三方API返回的歌词不需要解码或特殊处理
        Ok(lyric.to_string())
    }

    fn source_name(&self) -> &'static str {
        "Spotify"
    }
}

fn get_lyric_client() -> &'static LyricClient {
    static CLIENT: OnceLock<LyricClient> = OnceLock::new();
    CLIENT.get_or_init(|| LyricClient::new())
}

// 统一调用入口
#[derive(Default)]
struct LyricClient {
    fetchers: Vec<Box<dyn LyricFetcher>>,
    cache: CacheManager,
}

impl LyricClient {
    fn new() -> Self {
        Self {
            fetchers: vec![
                Box::new(NeteaseFetcher::default()),
                // Box::new(QQMusicFetcher::default()),
                // Box::new(KugouFetcher::new()),
                // Box::new(SpotifyFetcher::new()),
            ],
            cache: CacheManager::new(),
        }
    }

    async fn get_lyric(&self, song: &SongInfo) -> Result<Rope, LyricError> {
        if let Some(cached) = self.cache.get(song).await {
            log::debug!("Cache lyric for: {} - {}", song.artist, song.title);
            return Ok(cached);
        }

        for fetcher in &self.fetchers {
            log::debug!("Trying source: {}", fetcher.source_name());
            match fetcher.fetch_lyric(song).await {
                Ok(lyric) => {
                    //if self.validate_lyric(song, &lyric) {
                    log::info!("Successfully fetched from {}", fetcher.source_name());
                    self.cache
                        .store(song, fetcher.source_name(), &lyric)
                        .await?;
                    return Ok(Rope::from(lyric));
                    // }
                }
                Err(e) => log::warn!("{} failed: {}", fetcher.source_name(), e),
            }
        }
        Err(LyricError::NoLyricFound)
    }

    #[allow(dead_code)]
    fn validate_lyric(&self, song: &SongInfo, lyric: &str) -> bool {
        let normalized_lyric = normalize_text(lyric);
        let has_title = normalized_lyric.contains(&normalize_text(&song.title));
        let has_artist = normalized_lyric.contains(&normalize_text(&song.artist));

        // 额外检查时长标签（如果有）
        let has_duration = lyric.contains(&format!("{:0.1}", song.duration));

        has_title && has_artist && (song.duration <= 0.0 || has_duration)
    }
}

fn normalize_text(s: &str) -> String {
    s.to_lowercase()
        .replace([' ', '_', '-', '(', ')', '（', '）'], "")
        .trim()
        .to_string()
}

// 缓存管理模块
#[derive(Debug, Clone, Default)]
struct CacheManager {
    base_dir: PathBuf,
}

impl CacheManager {
    fn new() -> Self {
        let mut path = home_dir().expect("Failed to get home directory");
        path.push(".local/share/lyrics");

        if !path.exists() {
            fs::create_dir_all(&path).unwrap();
        }

        Self { base_dir: path }
    }

    fn lyric_name(&self, song: &SongInfo) -> PathBuf {
        let file_name = format!("{}_{}.lrc", sanitize(&song.artist), sanitize(&song.title));
        let mut path = self.base_dir.clone();
        path.push(file_name);
        path
    }

    async fn get(&self, song: &SongInfo) -> Option<Rope> {
        let path = self.lyric_name(song);
        if !path.exists() {
            return None;
        }

        tokio::fs::read_to_string(&path)
            .await
            .map(|s| Rope::from_str(&s))
            .ok()
    }

    async fn store(&self, song: &SongInfo, _source: &str, content: &str) -> Result<(), LyricError> {
        let path = self.lyric_name(song);
        tokio::fs::write(path, &content).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SongInfo {
    title: String,
    artist: String,
    duration: f64,
}

impl SongInfo {
    #[allow(dead_code)]
    fn normalized(&self) -> Self {
        Self {
            title: normalize_text(&self.title),
            artist: normalize_text(&self.artist),
            duration: 0.,
        }
    }
}

// 优化播放器查找逻辑
fn is_valid_player(player: &Player) -> bool {
    let identity = player.identity().to_lowercase();
    let blacklist_keywords = ["browser", "video", "screen-cast", "chromium", "firefox"];
    !blacklist_keywords.iter().any(|k| identity.contains(k))
}

fn get_current_song() -> Result<SongInfo, LyricError> {
    let player_finder = PlayerFinder::new()?;
    let player = player_finder
        .find_all()?
        .into_iter()
        .filter(|player| is_valid_player(&player))
        .max_by_key(|p| p.is_running()) // 优先选择正在播放的
        .ok_or_else(|| LyricError::NoPlayerFound)?;

    let metadata = player.get_metadata()?;

    // 获取所有活动播放器并过滤
    let title = metadata.title().unwrap_or_default().to_string();
    let artist = metadata.artists().map(|a| a.join(", ")).unwrap_or_default();
    let duration = metadata.length().map(|d| d.as_secs_f64()).unwrap_or(0.0);

    Ok(SongInfo {
        title,
        artist,
        duration,
    })
}

/// 播放时间
#[derive(Debug, Clone, PartialEq, Default)]
struct PlayTime {
    /// 当前时间
    current_time: f64,
    /// 最后校正时间
    last_valid_pos: Option<(Instant, f64)>,
}

fn get_current_time_song(st: PlayTime) -> Result<PlayTime, LyricError> {
    let player = PlayerFinder::new()?
        .find_active()
        .map_err(|_| LyricError::NoPlayerFound)?;
    let mut st = st;

    match player.get_position().map(|d| d.as_secs_f64()) {
        Ok(pos) => {
            st.current_time = pos;
            st.last_valid_pos = Some((Instant::now(), pos));
        }
        Err(_) => {
            // 根据最后一次有效位置和流逝时间估算
            if let Some((time, pos)) = st.last_valid_pos {
                let delta = Instant::now().duration_since(time).as_secs_f64();
                st.current_time = pos + delta;
            }
        }
    }

    Ok(st)
}

// 解析主逻辑
struct LyricParser;

impl LyricParser {
    pub fn parse(doc: &Rope, duration: f64) -> Result<Vec<LyricLine>, LyricError> {
        let mut entries = Vec::new();

        // 第一阶段：收集所有时间标签和文本
        for line in doc.lines() {
            let line_str = line.to_string();
            let (time_tags, text) = Self::parse_line(&line_str)?;

            for ts in time_tags {
                entries.push((ts, text.clone()));
            }
        }

        // 按时间排序
        entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // 第二阶段：创建带时间区间的歌词行
        let mut lyrics = Vec::with_capacity(entries.len());
        for (i, &(start, ref text)) in entries.iter().enumerate() {
            let end = entries
                .get(i + 1)
                .map(|(next_start, _)| *next_start)
                .unwrap_or(duration);

            lyrics.push(LyricLine {
                timestamp_start: start,
                timestamp_end: end,
                text: text.clone(),
            });
        }

        if lyrics.is_empty() {
            Err(LyricError::EmptyLyric)
        } else {
            Ok(lyrics)
        }
    }

    fn parse_line(line: &str) -> Result<(Vec<f64>, String), LyricError> {
        let mut line = line.trim();
        let mut time_tags = Vec::new();

        // 解析时间标签
        while line.starts_with('[') {
            let Some(end_idx) = line.find(']') else {
                break;
            };

            let time_str = &line[1..end_idx];
            line = &line[end_idx + 1..];

            match Self::parse_time(time_str) {
                Some(time) => time_tags.push(time),
                None => return Err(LyricError::InvalidTimeFormat(time_str.to_string())),
            }
        }

        // 添加有效歌词行
        let text = line.trim().to_string();

        Ok((time_tags, text))
    }

    fn parse_time(s: &str) -> Option<f64> {
        let parts: Vec<&str> = s.split(&[':', '.']).collect();
        if parts.len() < 2 {
            return None;
        }

        let minutes = parts[0].parse::<f64>().ok()?;
        let seconds = parts[1].parse::<f64>().ok()?;
        let millis = parts
            .get(2)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        Some(minutes * 60.0 + seconds + millis / 100.0)
    }
}

#[derive(Debug, Clone)]
struct LyricLine {
    timestamp_start: f64, // 单位：秒
    timestamp_end: f64,   // 单位：秒
    text: String,
}

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
struct AppState {
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
}

// 界面渲染
#[derive(Clone, Default)]
struct LyricWidget {
    state: AppState,
}

impl LyricWidget {
    async fn update(&mut self) {
        self.state.update().await;
    }

    fn update_size(&mut self, size: Size) {
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

#[derive(Clone, Default)]
pub struct App {
    counter: i32,
    exit: bool,

    lyric_widget: LyricWidget,
}

impl App {
    const FRAMES_PER_SECOND: f32 = 60.0;

    // 保持UI和主循环不变
    async fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
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
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Percentage(3), // 标题栏目
                Constraint::Min(1),        // 歌词区域
            ])
            .split(frame.area());

        // 渲染标题区块
        let title_block = Block::default()
            .borders(Borders::BOTTOM)
            .style(Style::default().fg(Color::LightBlue));
        frame.render_widget(title_block, layout[0]);

        let size = layout[1].as_size();
        self.lyric_widget.update_size(size);

        // 渲染到第一个子区域
        frame.render_widget(&self.lyric_widget, layout[1]);
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
            KeyCode::Esc => self.exit(),
            KeyCode::Left => self.decrement_counter(),
            KeyCode::Right => self.increment_counter(),
            _ => {}
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }

    fn increment_counter(&mut self) {
        self.counter += 1;
    }

    fn decrement_counter(&mut self) {
        self.counter -= 1;
    }
}

pub fn init_logger() -> Result<()> {
    // 日志文件路径（用户目录下的 .lyrics/logs/app.log）
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/lyrics");

    if !log_dir.exists() {
        fs::create_dir_all(&log_dir).unwrap();
    }

    let log_file = log_dir.join("lyric.log");

    // 配置日志输出到文件和终端
    env_logger::Builder::new()
        .format(|buf, record| {
            writeln!(
                buf,
                "[{} {} {}] {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.module_path().unwrap_or(""),
                record.args()
            )
        })
        .filter(None, log::LevelFilter::Trace) // 默认日志级别
        .target(env_logger::Target::Pipe(Box::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file)?,
        )))
        .try_init()?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logger()?;
    log::info!("Starting lyric application...");

    get_lyric_client();
    // color_eyre::install()?;
    let _args = Args::parse();
    let mut terminal = ratatui::init();
    let app_result = App::default().run(&mut terminal).await;
    ratatui::restore();
    app_result
}
