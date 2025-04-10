use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use super::HELP_KEY_STYLE;

#[derive(Clone, Default)]
pub(super) struct HelpScreen;

impl HelpScreen {
    // 帮助
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::new(
            Direction::Horizontal,
            [Constraint::Min(1), Constraint::Min(1), Constraint::Min(1)],
        );
        let [lyric_chunk, search_chunk, help_chunk] = chunks.areas(area);

        let lines = vec![
            ("    h | ? ", " 帮助."),
            ("  q | ESC ", " 退出."),
            ("d | Delete ", " 删除当前歌词"),
            ("      Left ", " 快退"),
            ("     Right ", "快进"),
            ("     n | j ", "下一曲"),
            ("     p | k ", "上一曲"),
        ];
        help(lines).render(lyric_chunk, buf);

        // search
        let lines = vec![("q | ESC ", " 退出到歌词界面.")];
        help(lines).render(search_chunk, buf);

        // help
        let lines = vec![
            ("q | ESC ", " 退出到歌词界面."),
            ("h | ?   ", " 帮助."),
            ("n | Down", "下一个"),
            ("p | Up  ", "上一个"),
            ("l | Enter ", "上一个"),
        ];
        help(lines).render(help_chunk, buf);
    }
}

// 提取的创建行函数
fn help<'a>(lines: Vec<(&'a str, &'a str)>) -> Paragraph<'a> {
    let lines: Vec<Line> = lines
        .into_iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(key, HELP_KEY_STYLE),
                Span::raw(":"),
                Span::raw(description),
            ])
        })
        .collect();

    Paragraph::new(lines)
        .block(Block::default().title("搜索").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}
