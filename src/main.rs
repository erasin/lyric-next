use anyhow::Result;
use base64::prelude::*;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode};
use dirs::home_dir;
use itertools::Itertools;
use mpris::{Metadata, PlayerFinder};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Widget},
};
use ropey::Rope;
use sanitize_filename::sanitize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use strum::{EnumIter, IntoEnumIterator};

mod error;
use error::LyricError;

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
    let path = MusicSource::iter()
        .filter_map(|source| get_cache_path(song, source).ok())
        .last();

    Ok(Rope::from_reader(fs::File::open(path.unwrap())?)?)
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
        fs::write(path, &lyric)?;
        Ok(Rope::from(lyric))
    } else {
        Err(LyricError::LyricValidationFailed)
    }
}

fn verify_lyric(song: &SongInfo, lyric: &str) -> bool {
    let normalized_lyric = normalize_text(lyric);
    normalized_lyric.contains(&normalize_text(&song.title))
        && normalized_lyric.contains(&normalize_text(&song.artist))
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

// QQ音乐实现
async fn fetch_qqmusic(song: &SongInfo) -> Result<String, LyricError> {
    let client = reqwest::Client::new();
    let search_url = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";

    let response = client
        .get(search_url)
        .query(&[
            ("w", format!("{} {}", song.title, song.artist)),
            ("format", "json".into()),
            ("n", "1".into()),
            ("ct", "24".into()),
            ("qqmusic_ver", "1298".into()),
            ("new_json", "1".into()),
            ("p", "1".into()),
            ("n", "5".into()),
        ])
        .header("Referer", "https://y.qq.com")
        .send()
        .await?;

    let json: Value = response.json().await?;
    let song_id = json["data"]["song"]["list"][0]["songid"]
        .as_str()
        .ok_or(LyricError::NoLyricFound)?;

    let lyric_url = format!(
        "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_yqq.fcg?songmid={}&format=json",
        song_id
    );
    let response = client
        .get(lyric_url)
        .header("Referer", "https://y.qq.com")
        .send()
        .await?;

    let json: Value = response.json().await?;
    let lyric = json["lyric"].as_str().ok_or(LyricError::NoLyricFound)?;

    let decoded = BASE64_STANDARD
        .decode(lyric)
        .map_err(|_| LyricError::LyricDecodeError)?;

    String::from_utf8(decoded).map_err(|_| LyricError::LyricDecodeError)
}

async fn fetch_kugou(song: &SongInfo) -> Result<String, LyricError> {
    let client = reqwest::Client::new();
    // 假设酷狗音乐的搜索API和参数如下（实际应使用真实的API和参数）
    let search_url = "https://example.com/kugou/search";
    let response = client
        .get(search_url)
        .query(&[
            ("keywords", format!("{} {}", song.title, song.artist)),
            // 其他可能的参数
        ])
        .send()
        .await?;

    let json: Value = response.json().await?;
    // 解析JSON以获取歌曲ID和歌词URL（实际实现取决于API的返回结构）
    let song_id = json["data"]["song_list"][0]["song_id"]
        .as_str()
        .ok_or(LyricError::NoLyricFound)?;
    let lyric_url = format!("https://example.com/kugou/lyric?song_id={}", song_id);

    let response = client.get(lyric_url).send().await?;
    let json: Value = response.json().await?;
    let lyric = json["lyric"].as_str().ok_or(LyricError::NoLyricFound)?;

    // 假设酷狗音乐的歌词不需要解码或特殊处理
    Ok(lyric.to_string())
}

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

fn get_current_song() -> Result<SongInfo, LyricError> {
    let player = PlayerFinder::new()?
        .find_active()
        .map_err(|_| LyricError::NoPlayerFound)?;

    let metadata = player.get_metadata()?;

    let title = metadata.title().unwrap_or_default().to_string();
    let artist = metadata.artists().map(|a| a.join(", ")).unwrap_or_default();
    let duration = metadata.disc_number().unwrap_or_default().into();

    Ok(SongInfo {
        title,
        artist,
        duration,
    })
}

// 解析主逻辑
struct LyricParser;

impl LyricParser {
    pub fn parse(doc: &Rope) -> Result<Vec<LyricLine>, LyricError> {
        let mut lines = Vec::new();

        for line in doc.lines() {
            Self::parse_line(&line.to_string(), &mut lines)?;
        }

        if lines.is_empty() {
            return Err(LyricError::EmptyLyric);
        }

        lines.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap());
        Ok(lines)
    }

    fn parse_line(line: &str, output: &mut Vec<LyricLine>) -> Result<(), LyricError> {
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
        let text = line.trim();
        if !text.is_empty() && !time_tags.is_empty() {
            for timestamp in time_tags {
                output.push(LyricLine {
                    timestamp,
                    text: text.to_string(),
                });
            }
        }

        Ok(())
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
    timestamp: f64, // 单位：秒
    text: String,
}

// 界面状态管理
struct AppState {
    current_song: Option<SongInfo>,
    lyrics: Vec<LyricLine>,
    last_update: Instant,
    scroll_offset: usize,
}

impl AppState {
    fn new() -> Self {
        Self {
            current_song: None,
            lyrics: Vec::new(),
            last_update: Instant::now(),
            scroll_offset: 0,
        }
    }

    async fn update(&mut self) -> Result<()> {
        let new_song = get_current_song()?;

        // 歌曲发生变化时重新加载歌词
        if Some(new_song) != self.current_song {
            self.handle_song_change(new_song.clone());
        }

        // 更新时间进度
        let delta = self.last_update.elapsed().as_secs_f64();
        self.last_update = Instant::now();
        self.update_progress(delta);

        if let Some(pos) = self.find_current_line() {
            self.scroll_offset = pos.saturating_sub(5); // 保持当前行在中间
        }

        Ok(())
    }

    async fn handle_song_change(&mut self, song: SongInfo) -> Result<()> {
        self.current_song = Some(song.clone());
        self.lyrics.clear();
        self.scroll_offset = 0;

        if let Some(song) = &self.current_song {
            let doc = fetch_lyric(&song).await?;
            self.lyrics = LyricParser::parse(&doc).expect("Failed to load lyrics for {song.title}");
        }

        Ok(())
    }

    fn find_current_line(&self) -> Option<usize> {
        self.lyrics
            .binary_search_by(|line| line.timestamp.partial_cmp(&self.last_update).unwrap())
            .ok()
    }

    fn total_duration(&self) -> Option<f64> {
        self.lyrics.last().map(|line| line.timestamp)
    }

    fn update_progress(&mut self, delta: f64) {
        if let Some(song) = &mut self.current_song {
            song.duration = (song.duration - delta).max(0.0);
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
        let inner_area = block.inner(area);
        block.render(area, buf);

        // 显示歌曲信息
        if let Some(song) = &self.0.current_song {
            let info_line = format!(
                " ♫ {} - {} [{:.1}s] ",
                song.artist, song.title, song.duration
            );
            buf.set_string(
                area.x + 1,
                area.y,
                &info_line,
                Style::default().fg(Color::LightBlue),
            );
        }

        let visible_lines = inner_area.height as usize;
        let start = self
            .0
            .scroll_offset
            .min(self.0.lyrics.len().saturating_sub(visible_lines));
        let end = (start + visible_lines).min(self.0.lyrics.len());

        for (i, line) in self.0.lyrics[start..end].iter().enumerate() {
            let y = inner_area.y + i as u16;
            let is_current = start + i == self.0.find_current_line().unwrap_or(0);

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

            let text = format!("{:6.1} {}", line.timestamp, line.text);
            buf.set_string(inner_area.x, y, &text, style);
        }
    }
}

// 保持UI和主循环不变
async fn run() -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut app_state = AppState::new();

    loop {
        // 更新状态
        app_state.update().await?;

        // 渲染界面
        terminal.draw(|f| {
            let chunks = Layout::default()
                .margin(1)
                .constraints([Constraint::Percentage(100)])
                .split(f.size());

            f.render_widget(LyricWidget(&app_state), chunks);
        })?;

        // 控制刷新率（每秒20帧）
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 处理退出
        if event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Esc {
                    break;
                }
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _args = Args::parse();
    run().await
}
