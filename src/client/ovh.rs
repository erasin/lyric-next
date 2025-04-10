use async_trait::async_trait;
use serde_json::Value;

use crate::{error::LyricError, song::SongInfo};

use super::{BaseFetcher, LyricFetcher};

// Spotify音乐实现
#[allow(dead_code)]
#[derive(Default)]
pub(super) struct OvhFetcher {
    base: BaseFetcher,
}

#[async_trait]
impl LyricFetcher for OvhFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        // 假设使用的第三方Spotify歌词API如下（实际应使用真实的API）
        let ovh_api = "https://api.lyrics.ovh/v1";

        let api_url = format!("{}/{}/{}", ovh_api, song.artist, song.title);

        // let encoded_artist = urlencoding::encode(&song.artist);
        // let encoded_title = urlencoding::encode(&song.title);
        let request = self
            .base
            .client
            .get(api_url)
            // .query(&[("track", &song.title), ("artist", &song.artist)])
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            );

        let json: Value = self.base.fetch_with_retry(request).await?;
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
