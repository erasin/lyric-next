use anyhow::Result;
use base64::prelude::*;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dirs::home_dir;
use mpris::{Player, PlayerFinder};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Widget},
};
use ropey::Rope;
use sanitize_filename::sanitize;
use serde_json::Value;
use std::{
    cmp::min,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use strum::{EnumIter, IntoEnumIterator};

use thiserror::Error;

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

#[derive(Clone, Copy, Debug, EnumIter, strum::Display)]
enum MusicSource {
    Netease,
    Kugou,
    QQ,
    Spotify,
}

fn normalize_text(s: &str) -> String {
    s.to_lowercase()
        .replace([' ', '_', '-', '(', ')', '（', '）'], "")
        .trim()
        .to_string()
}

async fn fetch_lyric(song: &SongInfo) -> Result<Rope, LyricError> {
    let normalized = song.normalized();

    // 尝试从缓存查找
    if let Ok(cached) = check_cached_lyrics(&normalized).await {
        return Ok(cached);
    }

    // 依次尝试不同来源
    for source in MusicSource::iter() {
        match try_fetch_from_source(&normalized, source).await {
            Ok(lyric) => return Ok(lyric),
            Err(e) => eprintln!("{} error: {}", source, e),
        }
    }

    Err(LyricError::NoLyricFound)
}

async fn check_cached_lyrics(song: &SongInfo) -> Result<Rope, LyricError> {
    let mut candidates: Vec<_> = MusicSource::iter()
        .filter_map(|source| {
            let path = get_cache_path(song, source).ok()?;
            if path.exists() {
                let modified = fs::metadata(&path).ok()?.modified().ok()?;
                Some((path, modified))
            } else {
                None
            }
        })
        .collect();

    // 按修改时间排序，选择最新的缓存
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    let path = candidates
        .first()
        .map(|(p, _)| p)
        .ok_or(LyricError::NoLyricFound)?;

    let content = tokio::fs::read_to_string(&path).await?;
    Ok(Rope::from_str(&content))
}

async fn try_fetch_from_source(song: &SongInfo, source: MusicSource) -> Result<Rope, LyricError> {
    let lyric = match source {
        MusicSource::Netease => fetch_netease(song).await,
        MusicSource::QQ => fetch_qqmusic(song).await,
        MusicSource::Kugou => fetch_kugou(song).await,
        MusicSource::Spotify => fetch_spotify(song).await,
    }?;

    if verify_lyric(song, &lyric) {
        let path = get_cache_path(song, source)?;
        tokio::fs::write(path, &lyric).await?;
        Ok(Rope::from(lyric))
    } else {
        Err(LyricError::LyricValidationFailed)
    }
}

fn verify_lyric(song: &SongInfo, lyric: &str) -> bool {
    let normalized_lyric = normalize_text(lyric);
    let has_title = normalized_lyric.contains(&normalize_text(&song.title));
    let has_artist = normalized_lyric.contains(&normalize_text(&song.artist));

    // 额外检查时长标签（如果有）
    let has_duration = lyric.contains(&format!("{:0.1}", song.duration));

    has_title && has_artist && (song.duration <= 0.0 || has_duration)
}

// 添加重试机制
async fn fetch_with_retry(
    client: &reqwest::Client,
    url: &str,
    retries: u8,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut attempt = 0;
    let mut backoff = 1;

    loop {
        match client.get(url).send().await {
            Ok(res) => return Ok(res),
            Err(e) if attempt < retries => {
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                attempt += 1;
                backoff *= 2;
            }
            Err(e) => return Err(e),
        }
    }
}
// 网易云实现
async fn fetch_netease(song: &SongInfo) -> Result<String, LyricError> {
    let client = reqwest::Client::new();
    let search_url = "https://music.163.com/api/search/get/";

    let response = client
        .get(search_url)
        .query(&[
            ("s", format!("{} {}", song.title, song.artist)),
            ("type", "1".into()),
            ("limit", "1".into()),
        ])
        .send()
        .await?;

    let json: Value = response.json().await?;
    let song_id = json["result"]["songs"][0]["id"]
        .as_u64()
        .ok_or(LyricError::NoLyricFound)?;

    let lyric_url = format!("https://music.163.com/api/song/lyric?id={}&lv=1", song_id);
    let response = client.get(lyric_url).send().await?;

    let json: Value = response.json().await?;
    json["lrc"]["lyric"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or(LyricError::NoLyricFound)
}

async fn fetch_qqmusic(song: &SongInfo) -> Result<String, LyricError> {
    let client = reqwest::Client::new();

    // 1. 搜索歌曲
    let search_url = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";
    let response = client
        .get(search_url)
        .query(&[
            ("w", format!("{} {}", song.title, song.artist)),
            ("format", "json".into()),
            ("n", "1".into()),
            ("cr", "1".into()),
            ("g_tk", "5381".into()),
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
    let response = client
        .get(lyric_url)
        .query(&[
            ("songmid", song_mid),
            ("format", "json".into()),
            ("g_tk", "5381".into()),
        ])
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

    String::from_utf8(decoded).map_err(|_| LyricError::LyricDecodeError)
}

async fn fetch_kugou(song: &SongInfo) -> Result<String, LyricError> {
    let client = reqwest::Client::new();

    // 1. 搜索歌曲
    let search_url = "http://mobilecdn.kugou.com/api/v3/search/song";
    let response = client
        .get(search_url)
        .query(&[
            ("keyword", format!("{} {}", song.title, song.artist)),
            ("page", "1".into()),
            ("pagesize", "1".into()),
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
    let response = client
        .get(lyric_url)
        .query(&[
            ("keyword", format!("{} {}", song.title, song.artist)),
            ("hash", song_hash.to_string()),
            ("album_id", album_id.to_string()),
            ("_", current_timestamp.to_string()),
        ])
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?;

    let json: Value = response.json().await?;
    let lyric = json["content"].as_str().ok_or(LyricError::NoLyricFound)?;

    // 处理酷狗特有的加密歌词
    let decoded = decode_kugou_lyric(lyric)?;
    Ok(decoded)
}

// 酷狗歌词解密函数
fn decode_kugou_lyric(encrypted: &str) -> Result<String, LyricError> {
    let bytes = BASE64_STANDARD.decode(encrypted)?;
    let key = b"kg@lrc$okm0qaz";
    let decrypted: Vec<u8> = bytes
        .iter()
        .enumerate()
        .map(|(i, &b)| b ^ key[i % key.len()])
        .collect();
    String::from_utf8(decrypted).map_err(|_| LyricError::LyricDecodeError)
}

// QQ音乐实现
// async fn fetch_qqmusic(song: &SongInfo) -> Result<String, LyricError> {
//     let client = reqwest::Client::new();
//     let search_url = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";

//     let response = client
//         .get(search_url)
//         .query(&[
//             ("w", format!("{} {}", song.title, song.artist)),
//             ("format", "json".into()),
//             ("n", "1".into()),
//             ("ct", "24".into()),
//             ("qqmusic_ver", "1298".into()),
//             ("new_json", "1".into()),
//             ("p", "1".into()),
//             ("n", "5".into()),
//         ])
//         .header("Referer", "https://y.qq.com")
//         .send()
//         .await?;

//     let json: Value = response.json().await?;
//     let song_id = json["data"]["song"]["list"][0]["songid"]
//         .as_str()
//         .ok_or(LyricError::NoLyricFound)?;

//     let lyric_url = format!(
//         "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_yqq.fcg?songmid={}&format=json",
//         song_id
//     );
//     let response = client
//         .get(lyric_url)
//         .header("Referer", "https://y.qq.com")
//         .send()
//         .await?;

//     let json: Value = response.json().await?;
//     let lyric = json["lyric"].as_str().ok_or(LyricError::NoLyricFound)?;

//     let decoded = BASE64_STANDARD
//         .decode(lyric)
//         .map_err(|_| LyricError::LyricDecodeError)?;

//     String::from_utf8(decoded).map_err(|_| LyricError::LyricDecodeError)
// }

// async fn fetch_kugou(song: &SongInfo) -> Result<String, LyricError> {
//     let client = reqwest::Client::new();
//     // 假设酷狗音乐的搜索API和参数如下（实际应使用真实的API和参数）
//     let search_url = "https://example.com/kugou/search";
//     let response = client
//         .get(search_url)
//         .query(&[
//             ("keywords", format!("{} {}", song.title, song.artist)),
//             // 其他可能的参数
//         ])
//         .send()
//         .await?;

//     let json: Value = response.json().await?;
//     // 解析JSON以获取歌曲ID和歌词URL（实际实现取决于API的返回结构）
//     let song_id = json["data"]["song_list"][0]["song_id"]
//         .as_str()
//         .ok_or(LyricError::NoLyricFound)?;
//     let lyric_url = format!("https://example.com/kugou/lyric?song_id={}", song_id);

//     let response = client.get(lyric_url).send().await?;
//     let json: Value = response.json().await?;
//     let lyric = json["lyric"].as_str().ok_or(LyricError::NoLyricFound)?;

//     // 假设酷狗音乐的歌词不需要解码或特殊处理
//     Ok(lyric.to_string())
// }

async fn fetch_spotify(song: &SongInfo) -> Result<String, LyricError> {
    let client = reqwest::Client::new();
    // 假设使用的第三方Spotify歌词API如下（实际应使用真实的API）
    let search_url = "https://api.thirdparty.com/spotify/lyrics";
    let response = client
        .get(search_url)
        .query(&[("track", &song.title), ("artist", &song.artist)])
        .send()
        .await?;

    let json: Value = response.json().await?;
    let lyric = json["lyrics"].as_str().ok_or(LyricError::NoLyricFound)?;

    // 假设第三方API返回的歌词不需要解码或特殊处理
    Ok(lyric.to_string())
}

// 更新缓存路径生成
fn get_cache_path(song: &SongInfo, source: MusicSource) -> Result<PathBuf, LyricError> {
    let mut path = home_dir().ok_or(LyricError::CachePathError)?;
    path.push(CACHE_DIR);

    if !path.exists() {
        fs::create_dir_all(&path)?;
    }

    let filename = match source {
        MusicSource::Netease => format!(
            "netease_{}_{}.lrc",
            sanitize(&song.artist),
            sanitize(&song.title)
        ),
        MusicSource::QQ => format!(
            "qqmusic_{}_{}.lrc",
            sanitize(&song.artist),
            sanitize(&song.title)
        ),
        MusicSource::Kugou => format!(
            "kugou_{}_{}.lrc",
            sanitize(&song.artist),
            sanitize(&song.title)
        ),
        MusicSource::Spotify => format!(
            "spotify_{}_{}.lrc",
            sanitize(&song.artist),
            sanitize(&song.title)
        ),
    };

    path.push(filename);
    Ok(path)
}

#[derive(Debug, Clone, PartialEq)]
struct SongInfo {
    title: String,
    artist: String,
    duration: f64,
}

impl SongInfo {
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

fn get_current_song() -> Result<(Player, SongInfo), LyricError> {
    // let player = PlayerFinder::new()?;
    // .find_active()
    // .map_err(|_| LyricError::NoPlayerFound)?;

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

    Ok((
        player,
        SongInfo {
            title,
            artist,
            duration,
        },
    ))
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
        let parts: Vec<&str> = s.split(|c| c == ':' || c == '.').collect();
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
    viewport_height: usize, // 视口高度
    scroll_range: usize,    // 最大可滚动范围
    line_height: u16,       // 单行高度
}
// 界面状态管理
struct AppState {
    player: Option<Player>,
    current_song: Option<SongInfo>,
    lyrics: Vec<LyricLine>,
    target_scroll: usize, // 目标滚动位置
    current_scroll: f64,  // 当前实际滚动位置
    current_time: f64,
    last_valid_pos: Option<(Instant, f64)>,
    view_metrics: ViewMetrics,     // 新增显示参数
    error_message: Option<String>, // 新增错误状态
    retry_counter: u32,            // 重试计数器
}

impl AppState {
    fn new() -> Self {
        Self {
            player: None,
            current_song: None,
            lyrics: Vec::new(),
            target_scroll: 0,
            current_scroll: 0.0,
            current_time: 0.0,
            last_valid_pos: None,
            view_metrics: ViewMetrics::default(),
            error_message: None,
            retry_counter: 0,
        }
    }

    // 预计算显示参数
    fn calculate_metrics(&mut self, area: Size) {
        let content_height = self.lyrics.len();
        let viewport_height = area.height as usize;
        let visible_lines = viewport_height.saturating_sub(2); // 保留边界空间
        let scroll_range = content_height.saturating_sub(visible_lines);

        self.view_metrics = ViewMetrics {
            visible_lines,
            content_height,
            viewport_height: viewport_height as usize,
            scroll_range,
            line_height: 1, // 假设单行高度为1
        };
    }

    async fn update(&mut self, area: Size) {
        self.error_message = None; // 清除旧错误        

        match self.try_update(area).await {
            Ok(_) => self.retry_counter = 0,
            Err(e) => {
                self.handle_error(e).await;
            }
        }
    }

    async fn try_update(&mut self, area: Size) -> Result<(), LyricError> {
        self.calculate_metrics(area); // 在更新时计算显示参数        
        // 获取当前播放器和歌曲信息
        let result = get_current_song();
        let (new_player, new_song) = match result {
            Ok((p, s)) => (Some(p), s),
            Err(LyricError::NoPlayerFound) => {
                self.player = None;
                self.current_song = None;
                self.lyrics.clear();
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };

        // 歌曲发生变化时重新加载歌词
        if Some(new_song.clone()) != self.current_song {
            self.handle_song_change(new_song).await?;
            self.player = new_player;
            self.calculate_metrics(area); // 歌词变化后重新计算
        }

        // 获取当前播放进度
        if let Some(player) = &self.player {
            match player.get_position().map(|d| d.as_secs_f64()) {
                Ok(pos) => {
                    self.current_time = pos;
                    self.last_valid_pos = Some((Instant::now(), pos));
                }
                Err(_) => {
                    // 根据最后一次有效位置和流逝时间估算
                    if let Some((time, pos)) = self.last_valid_pos {
                        let delta = Instant::now().duration_since(time).as_secs_f64();
                        self.current_time = pos + delta;
                    }
                }
            }
        }

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
            let doc = fetch_lyric(&song).await?;
            self.lyrics = LyricParser::parse(&doc, song.duration)
                .expect("Failed to load lyrics for {song.title}");
        }

        Ok(())
    }

    fn find_current_line(&self) -> Option<usize> {
        self.lyrics
            .iter()
            .enumerate()
            .find(|(_, line)| {
                self.current_time >= line.timestamp_start && self.current_time < line.timestamp_end
            })
            .map(|(i, _)| i)
    }

    async fn handle_error(&mut self, error: LyricError) {
        self.retry_counter += 1;
        let error_msg = format!("Error: {} (Retry {}/5)", error, self.retry_counter);
        self.error_message = Some(error_msg);

        // 自动重试逻辑
        if self.retry_counter < 5 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        } else {
            self.error_message = Some("Maximum retries reached".into());
        }
    }
}

// 界面渲染
struct LyricWidget<'a>(&'a AppState);

impl<'a> LyricWidget<'a> {
    fn get_window_title(&self) -> String {
        match &self.0.current_song {
            Some(song) => format!(" Now Playing: {} ", song.title),
            None => " No song playing ".into(),
        }
    }
}

impl<'a> Widget for LyricWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(self.get_window_title())
            .borders(Borders::ALL);
        block.render(area, buf);

        // 渲染错误信息
        if let Some(err_msg) = &self.0.error_message {
            let error_block = Paragraph::new(err_msg.clone())
                .style(Style::default().fg(Color::Red))
                .block(Block::default().borders(Borders::ALL));
            error_block.render(area, buf);
            return;
        }

        // 显示歌曲信息
        if let Some(song) = &self.0.current_song {
            let info_line = format!(
                " ♫ {} - {} {:0>2}:{:0>2} / {:0>2}:{:0>2}
                ",
                song.artist,
                song.title,
                (self.0.current_time / 60.0).floor() as u64,
                (self.0.current_time % 60.0).floor() as u64,
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
        let metrics = self.0.view_metrics;
        let scroll_pos = self.0.current_scroll as usize;
        let start = scroll_pos.min(metrics.scroll_range);
        let end = (start + metrics.visible_lines).min(metrics.content_height);
        for (i, line) in self.0.lyrics[start..end].iter().enumerate() {
            let y = area.y + i as u16;
            let is_current = start + i == self.0.find_current_line().unwrap_or(0);

            // 居中计算
            let line_text = format!(
                "[{:0>2}:{:0>2}] {}",
                (line.timestamp_start / 60.0).floor() as u64,
                (line.timestamp_start % 60.0).floor() as u64,
                line.text
            );

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

// 保持UI和主循环不变
async fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app_state = AppState::new();

    loop {
        // 获取当前终端尺寸
        let size = terminal.size()?;

        // 更新状态
        app_state.update(size).await;

        // 渲染界面
        terminal.draw(|frame| {
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

            // 渲染到第一个子区域
            frame.render_widget(LyricWidget(&app_state), layout[1]);
        })?;

        // 控制刷新率（每秒20帧）
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 处理退出
        if event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Esc {
                    break;
                }
                // 允许手动重试
                if key.code == KeyCode::Char('r') {
                    app_state.retry_counter = 0;
                }
            }
        }
    }

    ratatui::restore();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _args = Args::parse();
    run().await
}
