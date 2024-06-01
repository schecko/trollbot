
use anyhow::Context as _;
use std::error::Error;
use std::path::PathBuf;
use twitchchat::UserConfig;

pub fn data_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join("data"))
}

pub fn get_env_var(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("please set `{}`", key))
}

pub fn get_config<'a>() -> Result<UserConfig, Box<dyn Error>>
{
    let name = get_env_var("TWITCH_NAME")?;
    let token = get_env_var("TWITCH_TOKEN")?;

    let config = UserConfig::builder()
        // twitch account name
        .name(name)
        // OAuth token
        .token(token)
        .enable_all_capabilities()
        .build()?;

    Ok(config)
}

