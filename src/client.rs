use std::{
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use base64::{Engine, prelude::BASE64_STANDARD};
use ropey::Rope;
use serde_json::Value;

use crate::{cache::CacheManager, error::LyricError, song::SongInfo, utils::normalize_text};

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
                    tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
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

/// 初始client
pub fn get_lyric_client() -> &'static LyricClient {
    static CLIENT: OnceLock<LyricClient> = OnceLock::new();
    CLIENT.get_or_init(|| LyricClient::new())
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
                // Box::new(QQMusicFetcher::default()),
                // Box::new(KugouFetcher::new()),
                // Box::new(SpotifyFetcher::new()),
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
