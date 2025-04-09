use std::sync::OnceLock;

use async_trait::async_trait;
use kugou::KugouFetcher;
use netease::NeteaseFetcher;
use ovh::OvhFetcher;
use qqmusic::QQMusicFetcher;
use reqwest::RequestBuilder;
use ropey::Rope;
use serde_json::Value;

use crate::{cache::CacheManager, error::LyricError, song::SongInfo, utils::normalize_text};

mod kugou;
mod netease;
mod ovh;
mod qqmusic;

/// 歌词抓取器
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
    async fn fetch_with_retry(&self, request: RequestBuilder) -> Result<Value, LyricError> {
        let mut attempt = 0;
        loop {
            let response = request.try_clone().unwrap().send().await;
            match response {
                Ok(res) => return Ok(res.json().await?),
                Err(_e) if attempt < self.retries => {
                    tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// 初始client
pub fn get_lyric_client() -> &'static LyricClient {
    static CLIENT: OnceLock<LyricClient> = OnceLock::new();
    CLIENT.get_or_init(LyricClient::new)
}

// 统一调用入口
pub struct LyricClient {
    fetchers: Vec<Box<dyn LyricFetcher>>,
    pub cache: CacheManager,
}

impl LyricClient {
    fn new() -> Self {
        Self {
            fetchers: vec![
                Box::new(NeteaseFetcher::default()),
                Box::new(QQMusicFetcher::default()),
                Box::new(KugouFetcher::default()),
                Box::new(OvhFetcher::default()),
            ],
            cache: CacheManager::new(),
        }
    }

    pub async fn get_lyric(&self, song: &SongInfo) -> Result<Rope, LyricError> {
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
