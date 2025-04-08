use async_trait::async_trait;
use serde_json::Value;

use crate::{error::LyricError, song::SongInfo};

use super::{BaseFetcher, LyricFetcher};

// 网易云音乐实现
#[derive(Default)]
pub(super) struct NeteaseFetcher {
    base: BaseFetcher,
}

#[async_trait]
impl LyricFetcher for NeteaseFetcher {
    async fn fetch_lyric(&self, song: &SongInfo) -> Result<String, LyricError> {
        log::debug!("Netease song: {:?}", song);
        let search_url = "https://music.163.com/api/search/get/";

        let request = self.base.client.get(search_url).query(&[
            ("s", format!("{} {}", song.title, song.artist)),
            ("type", "1".into()),
            ("limit", "1".into()), // song_id 1, album_id 10 playlist_id 1000
            ("offset", "0".into()),
        ]);

        let json = self.base.fetch_with_retry(request).await?;
        log::debug!("Get song: {:?}", json);

        let song_id = json["result"]["songs"][0]["id"]
            .as_u64()
            .ok_or(LyricError::NoLyricFound)?;

        let lyric_url = format!("https://music.163.com/api/song/lyric?id={}&lv=1", song_id);
        let request = self
            .base
            .client
            .get(lyric_url)
            .query(&[("id", song_id), ("lv", 1)]);

        let json: Value = self.base.fetch_with_retry(request).await?;

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
