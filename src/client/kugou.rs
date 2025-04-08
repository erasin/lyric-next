use async_trait::async_trait;
use base64::{Engine, prelude::BASE64_STANDARD};
use serde_json::Value;

use crate::{error::LyricError, song::SongInfo};

use super::{BaseFetcher, LyricFetcher};

// Kugou音乐实现
#[derive(Default)]
pub(super) struct KugouFetcher {
    base: BaseFetcher,
}

impl KugouFetcher {
    // 酷狗歌词解密函数
    fn decode_lyric(&self, encrypted: &str) -> Result<String, LyricError> {
        let bytes = BASE64_STANDARD.decode(encrypted)?;
        let re = String::from_utf8(bytes).map_err(|_| LyricError::LyricDecodeError)?;
        if re.is_empty() {
            return Err(LyricError::NoLyricFound);
        }
        Ok(re)
    }
}

#[async_trait]
impl LyricFetcher for KugouFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        log::debug!("kugou start ");

        // 1. 搜索歌曲
        let search_url = "http://mobilecdn.kugou.com/api/v3/search/song";
        let request = self.base.client.get(search_url).query(&[
            (
                "keyword",
                format!("{} {}", song.title, song.artist).as_str(),
            ),
            ("page", "1"),
            ("pagesize", "1"),
        ]);
        let json: Value = self.base.fetch_with_retry(request).await?;
        log::debug!("song json: {json}");

        let song_hash = json["data"]["info"][0]["hash"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;
        let album_id = json["data"]["info"][0]["album_id"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;

        log::debug!("song hash: {song_hash}");

        // 2. 获取歌词
        let lyric_url = "http://krcs.kugou.com/search";
        let request = self
            .base
            .client
            .get(lyric_url)
            .query(&[
                ("hash", song_hash),
                ("album_id", album_id),
                ("ver", "1"),
                ("client", "pc"),
                ("man", "yes"),
            ])
            .header("User-Agent", "Mozilla/5.0");

        let json: Value = self.base.fetch_with_retry(request).await?;

        let download_id = json["candidates"][0]["download_id"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;

        let access_key = json["candidates"][0]["accesskey"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;

        log::debug!("song id: {download_id} , {access_key}");

        // 3. 下载
        let lyric_download_url = "http://lyrics.kugou.com/download";
        let request = self
            .base
            .client
            .get(lyric_download_url)
            .query(&[
                ("ver", "1"),
                ("client", "pc"),
                ("fmt", "lrc"),
                ("charset", "utf8"),
                ("accesskey", access_key),
                ("id", download_id),
            ])
            .header("User-Agent", "Mozilla/5.0");

        let json: Value = self.base.fetch_with_retry(request).await?;
        let lyric = json["content"].as_str().ok_or(LyricError::NoLyricFound)?;
        let decoded = self.decode_lyric(lyric)?;
        Ok(decoded)
    }

    fn source_name(&self) -> &'static str {
        "Kugou"
    }
}
