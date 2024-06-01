
use crate::file_help::load_file;
use crate::list::parse_list;

use anyhow::Context as _;
use std::error::Error;
use std::path::PathBuf;
use twitchchat::UserConfig;

pub fn data_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join("data"))
}

pub fn config_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join("config"))
}

pub fn user_dir() -> anyhow::Result<PathBuf> {
    let mut path = dirs::home_dir().unwrap();
    path.push("cynobot");
    Ok(path)
}

pub fn get_env_var(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("please set `{}`", key))
}

pub fn get_config<'a>(channel_content: &'a str) -> Result<(UserConfig, Vec<&'a str>), Box<dyn Error>> {
    let name = get_env_var("TWITCH_NAME")?;
    let token = get_env_var("TWITCH_TOKEN")?;
    let channels = parse_list(channel_content);

    let config = UserConfig::builder()
        // twitch account name
        .name(name)
        // OAuth token
        .token(token)
        .enable_all_capabilities()
        .build()?;

    Ok((config, channels))
}

pub fn load_config_file(name: &str) -> anyhow::Result<String> { 
    let full_path = config_dir()?.join(name);
    load_file(&full_path)
}

pub fn load_file_rel(name: &str) -> anyhow::Result<String> { 
    let full_path = data_dir()?.join(name);
    load_file(&full_path)
}

