
use crate::config::*;
use crate::file_help::*;
use crate::list::*;

use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::error::Error;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use twitchchat::UserConfig;
use twitchchat::commands;
use twitchchat::connector;
use twitchchat::messages;
use twitchchat::runner::AsyncRunner;
use twitchchat::runner::Status;
use unicode_properties::GeneralCategoryGroup;
use unicode_properties::UnicodeGeneralCategory;

const INPUT_CHANNELS: &str = "input_channels.list";

#[allow(dead_code)]
struct TokenIndex(u32);

#[derive(Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_name: String,
}

impl ChannelState { 
    fn new(channel_name: &str) -> Self {
        Self { 
            channel_name: String::from(channel_name),
        } 
    }

	#[allow(dead_code)]
    async fn send_message(&mut self, runner: &mut AsyncRunner, msg: &str) {
        println!("sending message [{}] to [{}]", msg, self.channel_name);
        let mut writer = runner.writer();
        let cmd = commands::privmsg(&self.channel_name, msg);
        writer.encode(cmd).await.unwrap();
    }
}

#[derive(Deserialize, Serialize)]
pub struct State
{
    // TODO: improve. hash is from channel name... just don't want to allocate every query...
    pub channels: HashMap<u64, ChannelState>,
}

impl State
{
    fn new(channels: Vec<&str>) -> Self
    {
        println!("creating new state");
        let chans = channels
            .iter()
            .map(|&chan| { 
            (
                State::chash(chan), 
                ChannelState::new(chan),
            ) } )
            .collect();

        State {
            channels: chans,
        }
    }

    fn chash(channel_name: &str) -> u64 {
        // TODO ensure the hash is stable for io
        let mut s = std::collections::hash_map::DefaultHasher::new();
        channel_name.hash(&mut s);
        s.finish()
    }

	#[allow(dead_code)]
    fn merge(channels: Vec<&str>, mut state: State) -> State
    {
        for channel in channels {
            state.channels.entry(Self::chash(channel)).or_insert_with(|| ChannelState::new(channel));
        }
        state
    }
}

pub fn config_dir() -> anyhow::Result<PathBuf>
{
    println!("config_dir");
    Ok(std::env::current_dir()?.join("pplm_config").canonicalize()?)
}

#[allow(dead_code)]
pub fn user_dir() -> anyhow::Result<PathBuf>
{
    println!("user_dir");
    let mut path = dirs::home_dir().unwrap();
    path.push("pplm_cynobot");
    Ok(path)
}

pub fn load_config_file(name: &str) -> anyhow::Result<String> { 
    println!("load_config_file -- {}", name);
    let full_path = config_dir()?.join(name);
    println!("load_config_file -- {}", full_path.display());
    load_file(&full_path)
}

#[allow(dead_code)]
pub fn load_file_rel(name: &str) -> anyhow::Result<String> { 
    println!("load_file_rel -- {}", name);
    let full_path = data_dir()?.join(name);
    load_file(&full_path)
}

async fn connect(user_config: &UserConfig, channels: &Vec<&str>) -> anyhow::Result<AsyncRunner>
{
    let connector = connector::tokio::ConnectorRustTls::twitch()?;

    println!("Connecting...");
    let mut runner = AsyncRunner::connect(connector, user_config).await?;
    println!("..Connected");

    for channel in channels {
        let _ = runner.join(&channel).await?;
        println!("joined '{}'!", channel);
    }

    Ok(runner)
}

pub async fn main_loop(mut state: State, mut runner: AsyncRunner) -> Result<(), Box<dyn Error>>
{
    loop {
        match runner.next_message().await? {
            Status::Message(msg) => {
                handle_message(&mut state, &mut runner, msg).await;
            }
            Status::Quit => {
                println!("Quitting.");
                break;
            }
            Status::Eof => {
                println!("Eof");
                break;
            }
        }
    }

    Ok(())
}

async fn connect_run() -> Result<(), Box<dyn Error>>
{
    let channels_content = load_config_file(INPUT_CHANNELS)?;
    let user_config = get_config()?;
    let channels = parse_list(&channels_content);

    let runner = connect(&user_config, &channels).await?;
    println!("starting main loop"); 

    // TODO
    let state = State::new(vec![]);

    main_loop(state, runner).await 
}

fn tokenize_message(msg: &messages::Privmsg<'_>) -> Vec<String>
{
    let mut tokens = Vec::new();
    let mut wip_token = String::new();
    for c in msg.data().chars()
    {
        let group = c.general_category_group();
        match (group, c)
        {
			(GeneralCategoryGroup::Letter, _) |
			(GeneralCategoryGroup::Number, _) |
			(_, '\'') =>
            {
				wip_token.push(c);
                continue;
            }
			(GeneralCategoryGroup::Mark, _) |
			(GeneralCategoryGroup::Punctuation, _) |
			(GeneralCategoryGroup::Symbol, _) |
			(GeneralCategoryGroup::Separator, _) |
			(GeneralCategoryGroup::Other, _) =>
            {
				if !wip_token.is_empty()
				{
					tokens.push(wip_token);
					wip_token = String::new();
				}
				tokens.push(String::from(c));
                continue;
            }
		}
    } 

    if !wip_token.is_empty()
    {
        tokens.push(wip_token);
    }

    tokens
}

async fn handle_message(_state: &mut State, _runner: &mut AsyncRunner, msg: messages::Commands<'_>)
{
    use messages::Commands::*;
    match msg {
        Privmsg(msg) => {
            let channel = &msg.channel()[1..]; // strip the #
            println!("[{}] {}: {}", channel, msg.name(), msg.data());
            // TODO
            let tokens = tokenize_message(&msg);
            println!("tokenized: {}", tokens.join("|"));

        },

        // unimplemented features from crate twitchchat
        Raw(_) => {}

        // initial connection events
        IrcReady(_) => {}
        Ready(_) => {}
        Cap(_) => {}

        // other events
        ClearChat(_) => {}
        ClearMsg(_) => {}
        GlobalUserState(_) => {}
        HostTarget(_) => {}
        Join(_) => {}
        Notice(_) => {}
        Part(_) => {}
        Ping(_) => {}
        Pong(_) => {}
        Reconnect(_) => {}
        RoomState(_) => {}
        UserNotice(_) => {}
        UserState(_) => {}
        Whisper(_) => {}

        _ => {}
    }
}

pub async fn main()
{ 
    let mut last_start_time = SystemTime::now();
    let mut fail_count = 0;
    loop {
        let start_time = SystemTime::now();
        match connect_run().await {
            Ok(_) => {}
            Err(e) => {
                println!("error in main {:?}", e);
            }
        }
        if start_time.duration_since(last_start_time).unwrap() < Duration::from_secs( 60 * 60 ) {
            fail_count += 1;
        } else {
            fail_count = 0;
        }
        let sleep_duration = Duration::from_secs(2u64.pow(fail_count));
        println!("disconnected, reconnecting in {:?}", sleep_duration);
        std::thread::sleep(sleep_duration);
        last_start_time = start_time;
    }
}
