use async_trait::async_trait;
use base64::{Engine, prelude::BASE64_STANDARD};
use serde_json::Value;

use crate::{error::LyricError, song::SongInfo};

use super::{BaseFetcher, LyricFetcher};

// QQ音乐实现
#[derive(Default)]
pub(super) struct QQMusicFetcher {
    base: BaseFetcher,
}

#[async_trait]
impl LyricFetcher for QQMusicFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        log::debug!("QQ search");

        // 1. 搜索歌曲
        let search_url = "https://c.y.qq.com/soso/fcgi-bin/client_search_cp";
        let request= self
            .base
            .client
            .get(search_url)
            .query(&[
                ("w",  format!("{} {}", song.title, song.artist).as_str()),
                ("format", "json"),
                ("p","1"), // page
                ("n", "1"),// 每页数量
                ("cr", "1"), // 中文
                ("t","0") // 搜索类型 0 歌曲
                // ("g_tk", "5381"), //
            ])
            .header("Referer", "https://y.qq.com/n/ryqq/player")
            .header("Host", "c.y.qq.com")
            .header("Origin", "https://y.qq.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/102.0.5005.63 Safari/537.36")            ;

        let json: Value = self.base.fetch_with_retry(request).await?;
        let song_mid = json["data"]["song"]["list"][0]["songmid"]
            .as_str()
            .ok_or(LyricError::NoLyricFound)?;

        log::debug!("song mid : {song_mid}");
        // 2. 获取歌词

        let lyric_url = "https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg";
        let request = self
            .base
            .client
            .get(lyric_url)
            .query(&[("songmid", song_mid), ("format", "json"), ("g_tk", "5381")])
            .header("Referer", "https://y.qq.com/n/ryqq/player")
            .header("Host", "c.y.qq.com")
            .header("Origin", "https://y.qq.com");

        let json: Value = self.base.fetch_with_retry(request).await?;
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
