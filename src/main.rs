use anyhow::Result;
use chrono::Local;
use clap::Parser;
use lyric_next::cache::CACHE_DIR;
// use color_eyre::Result;
use lyric_next::{app::App, client::get_lyric_client};
use std::io::Write;
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
};

#[derive(Parser, Debug)]
#[clap(version, about)]
struct Args;

pub fn init_logger() -> Result<()> {
    // 日志文件路径（用户目录下的 .lyrics/logs/app.log）
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CACHE_DIR);

    if !log_dir.exists() {
        fs::create_dir_all(&log_dir).unwrap();
    }

    let log_file = log_dir.join("lyric.log");

    #[cfg(debug_assertions)]
    let level = log::LevelFilter::Trace;

    #[cfg(not(debug_assertions))]
    let level = log::LevelFilter::Info;

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
        .filter(None, level) // 默认日志级别
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
    let _args = Args::parse();
    let mut terminal = ratatui::init();
    let app_result = App::default().run(&mut terminal).await;
    ratatui::restore();
    app_result
}
