// TODO 
// - per trigger cooldowns
extern crate tokio;
extern crate anyhow;
extern crate rand;
extern crate strum;

use twitchchat::{
    commands, connector, messages,
    runner::{AsyncRunner, Status},
    UserConfig,
};
use anyhow::Context as _;
use std::time::{ Duration, Instant };
use rand::Rng;
use strum::*;
use std::collections::{ HashMap, HashSet };
use std::fs::{ self, File };
use std::io::prelude::*;
use std::path::{ Path, PathBuf };
use std::error::Error;
use std::borrow::Cow;

const TRIGGERS_FILE: &str = "triggers.map";
const CONFIG_CHANNELS: &str = "channels.list";

fn parse_list<'a>(contents: &'a str) -> Vec<&'a str> {
    let mut data = Vec::new();
    for line in contents.lines() {
        if line.len() == 0 { continue; }
        if let Some('-') = line.chars().next() {
            continue;
        }
        data.push(line); 
    } 
    data
}

#[derive(Debug)]
pub enum MapValue<'a> {
    FileName(&'a str),
    Value(&'a str),
}

#[derive(Debug)]
pub struct MultiTrigger<'a> {
    triggers: [&'a str; 4],
    value: MapValue<'a>, 
}

// limitation: keys generated from values that contain capitals will never be tolowered, so those
// keys will always fail to compare
fn load_map<'a>(contents: &'a str, lists: &HashMap<&'a str, Vec<&'a str>>) -> (Vec<MultiTrigger<'a>>, HashMap<Cow<'a, str>, MapValue<'a>>) {
    let mut map = HashMap::new();
    let mut multi_triggers = Vec::new();
    for line in contents.lines() {
        let mut split = line.split('='); 
        if let (Some(meta_key), Some(value)) = (split.next(), split.next()) {
            if meta_key.len() == 0 { continue; }
            if value.len() == 0 { continue; }
            if let Some('-') = meta_key.chars().next() {
                continue;
            }

            // the starting character can be a meta key, if the meta_key is a forward square
            // bracket, then the key is pointing to a list file. treat each entry as a key
            let single = vec![meta_key];
            let keys = if let Some('[') = meta_key.chars().next() {
                lists.get(&meta_key[1..]).unwrap()
            } else {
                &single
            };

            'key_loop: for key in keys { 
                let map_value = if let Some('[') = value.chars().next() {
                    MapValue::FileName(&value[1..]) 
                } else {
                    MapValue::Value(value) 
                };

                if key.contains(' ') {
                    let mut multi_split = key.split(' ');

                    let first = multi_split.next();
                    let second = multi_split.next();
                    if first == None { continue 'key_loop; }
                    if second == None { continue 'key_loop; }

                    multi_triggers.push(MultiTrigger { 
                        triggers: [
                            first.unwrap(),
                            second.unwrap(),
                            multi_split.next().unwrap_or(""),
                            multi_split.next().unwrap_or(""),
                        ],
                        value: map_value,
                    }); 
                } else {
                    if key.contains('{') { continue 'key_loop; }
                    map.insert(subst_global(Cow::Borrowed(*key)), map_value);
                }
            }
        }
    } 
    (multi_triggers, map)
}

const PASSIVE_MESSAGES: bool = true;
const TRIGGER_MESSAGES: bool = true;
const COMMAND_MESSAGES: bool = true;

async fn connect(user_config: &UserConfig, channels: &Vec<&str>) -> anyhow::Result<AsyncRunner> {
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

fn data_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join("data"))
}

fn load_file_rel(name: &str) -> anyhow::Result<String> { 
    let full_path = data_dir()?.join(name);
    load_file(&full_path)
}

fn config_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join("config"))
}

fn load_config_file(name: &str) -> anyhow::Result<String> { 
    let full_path = config_dir()?.join(name);
    load_file(&full_path)
}

fn load_file(full_path: &Path) -> anyhow::Result<String> {
    println!("path {:?}", full_path);
    let mut file = File::open(full_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

async fn connect_run() -> Result<(), Box<dyn Error>> {
    let channels_content = load_config_file(CONFIG_CHANNELS)?;
    let (user_config, channels) = get_config(&channels_content)?;

    let runner = connect(&user_config, &channels).await?;
    println!("starting main loop"); 

    let dir = fs::read_dir(data_dir()?)?;
    let mut contents = Vec::new();
    let mut map = HashMap::new();
    for file in dir.filter_map(|file| file.ok()) {
        let path = file.path();
        if let Some(ext) = path.extension() {
            if ext != "list" { continue; }
            let content = load_file(&path)?;
            contents.push((String::from(path.file_stem().unwrap().to_str().unwrap()), content));
        }
    } 

    let contents = contents;
    for content in &contents {
        map.insert(&content.0[..], parse_list(&content.1)); 
    }

    let triggers_content = load_file_rel(TRIGGERS_FILE)?;
    let (multi_triggers, triggers) = load_map(&triggers_content, &map); 

    println!("lists {:#?}", map);
    println!("multi triggers {:#?}", multi_triggers);
    println!("triggers {:#?}", triggers);

    let state = State::new(channels);
    let lm = ListsMaps::new( map, multi_triggers, triggers);

    main_loop(state, &lm, runner).await 
}

#[tokio::main]
async fn main() { 
    loop {
        match connect_run().await {
            Ok(_) => {}
            Err(e) => {
                println!("error in main {:?}", e);
            }
        }
    }
}

fn get_env_var(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("please set `{}`", key))
}

pub fn get_config<'a>(channel_content: &'a str) -> Result<(twitchchat::UserConfig, Vec<&'a str>), Box<dyn Error>> {
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

const PASSIVE_ADVICE_INTERVAL: Duration = Duration::from_secs(60 * 30); // 30min
const BACKOFF_ADVICE_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24); // 24h

#[derive(Display, PartialEq, Eq)]
pub enum Mood {
    #[strum(to_string = "normal")]
    Normal,
    #[strum(to_string = "agitated")]
    Backoff,
}

pub struct ChannelState<'a> {
    pub mood: Mood,
    pub last_advice: Instant,
    pub next_advice: Duration,
    pub dedup_message: bool, 
    pub channel_name: &'a str,
}

impl<'a> ChannelState<'a> { 
    fn set_mood(&mut self, mood: Mood) {
        self.mood = mood;
    }

    async fn send_message(&mut self, runner: &mut AsyncRunner, msg: &str) {
        let mut writer = runner.writer();
        let cmd = commands::privmsg(&self.channel_name, msg);
        writer.encode(cmd).await.unwrap();

        self.dedup_message = true;
        self.last_advice = Instant::now();
    }
}

pub struct ListsMaps<'a> {
    pub lists: HashMap<&'a str, Vec<&'a str>>,
    pub multi_triggers: Vec<MultiTrigger<'a>>,
    pub triggers: HashMap<Cow<'a, str>, MapValue<'a>>,
}

impl<'a> ListsMaps<'a> {
    fn new(
        lists: HashMap<&'a str, Vec<&'a str>>,
        multi_triggers: Vec<MultiTrigger<'a>>, 
        triggers: HashMap<Cow<'a, str>, MapValue<'a>>, 
    ) -> Self {
        ListsMaps {
            lists,
            multi_triggers,
            triggers,
        } 
    }

}

pub struct State<'a> {
    pub channels: HashMap<&'a str, ChannelState<'a>>,
    pub ignores: HashSet<String>,
}

impl<'a> State<'a> {
    fn new(
        channels: Vec<&'a str>, 
    ) -> Self {
        let chans = channels
                .iter()
                .map(|&chan| { 
                (
                    chan, 
                    ChannelState { 
                        mood: Mood::Normal, 
                        last_advice: Instant::now(), 
                        next_advice: PASSIVE_ADVICE_INTERVAL, 
                        dedup_message: false,
                        channel_name: chan,
                    } 
                ) } )
                .collect();
        State {
            channels: chans,
            ignores: HashSet::new(),
        }
    }

    fn set_mood(&mut self, channel: &str, mood: Mood) {
        if let Some( channel_state ) = self.channels.get_mut( channel ) {
            channel_state.mood = mood;
        }
    } 
}

async fn send_passive_advice(state: &mut ChannelState<'_>, lm: &ListsMaps<'_>, runner: &mut AsyncRunner) {
    let passive = lm.lists.get("passive_advice").unwrap();
    let mut rng = rand::thread_rng();
    let msg = passive[rng.gen::<usize>() % passive.len()]; 
    let result = substitute_random(lm, msg); 
    state.send_message(runner, &result).await 
}

async fn send_passive_question(state: &mut ChannelState<'_>, lm: &ListsMaps<'_>, runner: &mut AsyncRunner) {
    let passive = lm.lists.get("questions").unwrap();
    let mut rng = rand::thread_rng();
    let msg = passive[rng.gen::<usize>() % passive.len()]; 
    let result = substitute_random(lm, msg); 
    state.send_message(runner, &result).await 
}

pub async fn main_loop(mut state: State<'_>, lm: &ListsMaps<'_>, mut runner: AsyncRunner) -> Result<(), Box<dyn Error>> {
    loop {
        match runner.next_message().await? {
            Status::Message(msg) => {
                handle_message(&mut state, lm, &mut runner, msg).await;
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

        for ( _channel, cstate ) in &mut state.channels {
            if cstate.last_advice + cstate.next_advice < Instant::now() {
                match cstate.mood {
                    Mood::Normal => {
                        if PASSIVE_MESSAGES && !cstate.dedup_message { 
                            send_passive_advice(cstate, lm, &mut runner).await;
                        }

                    }
                    Mood::Backoff => {
                        cstate.set_mood(Mood::Normal);
                        cstate.next_advice = PASSIVE_ADVICE_INTERVAL;
                    }
                }
            }
        }
    }

    Ok(())
}

struct SubLocations<'a> {
    original: &'a str,
    substr: &'a str, 
    acc: usize,
}

impl<'a> SubLocations<'a> {
    fn new(data: &'a str) -> Self {
        SubLocations {
            original: data,
            substr: data,
            acc: 0
        }
    }
}

impl<'a> Iterator for SubLocations<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(left_bracket) = self.substr.find('{') {
            if let Some(right_bracket) = self.substr[left_bracket..].find('}') {
                let result = &self.original[self.acc + left_bracket..self.acc + left_bracket + right_bracket + 1];
                self.substr = &self.substr[right_bracket..];
                self.acc += right_bracket;
                return Some(result);
            } 
        }
        None
    }
}

fn substitute_random<'a>(lm: &ListsMaps<'a>, message: &'a str) -> Cow<'a, str> { 
    if message.contains("{") {
        println!("substituting {}", message);
        let mut result = String::from(message);
        for substitution in SubLocations::new(message) {
            println!("found substitution location {}", substitution);
            if substitution.len() < 3 { continue; } 
            if let Some(list) = lm.lists.get(&substitution[1..substitution.len() - 1]) {
                let mut rng = rand::thread_rng();
                let msg = list[rng.gen::<usize>() % list.len()];
                println!("substituting {} for {}", substitution, msg);
                result = result.replace(substitution, msg); 
                println!("intermediate sub {}", result);
            }
        }
        Cow::Owned(result)
    } else {
        Cow::Borrowed(message)
    }
}

fn subst_global<'a>(message: Cow<'a, str>) -> Cow<'a, str> {
    if message.contains("{") {
        let result = message.replace("{me}", "somewhatinaccurate"); // TODO get this from somewhere
        return Cow::Owned(result);
    } else {
        return message;
    } 
}

fn subst_context<'a>(state: &ChannelState<'_>, user: &str, trigger: &str, message: Cow<'a, str>) -> Cow<'a, str> { 
    if message.contains("{") {
        let mut result = message.replace("{trigger}", trigger);
        result = result.replace("{user}", user);
        result = result.replace("{channel}", state.channel_name);
        return subst_global(Cow::Owned(result));
    } else {
        return message;
    } 
}

fn make_response<'a>(state: &ChannelState<'_>, lm: &ListsMaps<'a>, user: &str, trigger: &str, map_value: &MapValue<'a>) -> Option<Cow<'a, str>> {
    match map_value {
        MapValue::FileName(name) => {
            if let Some(list) = lm.lists.get(*name) {
                let mut rng = rand::thread_rng();
                let msg = list[rng.gen::<usize>() % list.len()];
                println!("detected file {}", name);
                let mut result = substitute_random(lm, msg);
                result = subst_context(state, user, trigger, result);
                return Some(result);
            }
        }
        MapValue::Value(value) => {
            println!("detected value {}", value);
            let mut result = substitute_random(lm, value);
            result = subst_context(state, user, trigger, result);
            return Some(result);
        }
    } 
    return None;
}

async fn handle_triggers(state: &mut State<'_>, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, msg: &messages::Privmsg<'_>) -> anyhow::Result<()> {
    let channel = &msg.channel()[1..]; // strip the #
    if let Some( cstate ) = state.channels.get_mut( channel ) {
        if cstate.mood == Mood::Normal && !state.ignores.contains(msg.name()) { 
            let lower_case = msg.data().to_lowercase();
            // todo ignore punctuation?
            for token in lower_case.split_whitespace() {
                match lm.triggers.get(token) {
                    Some(value) => {
                        if let Some(response) = make_response(cstate, lm, msg.name(), token, value) {
                            cstate.send_message(runner, &response).await; 
                        }
                    }
                    _ => {}
                }
            }

            let mut opt_response = None;
            'outer: for multi_trigger in &lm.multi_triggers {
                let mut found = false;
                'inner: for trigger in &multi_trigger.triggers {
                    if *trigger == "" { 
                        if found { 
                            break 'inner;
                        } else {
                            continue 'outer; 
                        }
                    }

                    let trigger_subst = subst_context(cstate, msg.name(), "", Cow::Borrowed(trigger)); 
                    if lower_case.contains(&*trigger_subst) {
                        found = true; 
                    } else {
                        found = false;
                        break 'inner;
                    }
                }

                if found { 
                    opt_response = make_response(cstate, lm, msg.name(), &multi_trigger.triggers.join(" "), &multi_trigger.value);
                }
            }
            if let Some(response) = opt_response {
                cstate.send_message(runner, &response).await; 
            } 
        }
    }
    Ok(())
}


async fn parse_command(state: &mut State<'_>, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, msg: &messages::Privmsg<'_>) -> Result<(), Box<dyn Error>> {
    let channel = &msg.channel()[1..]; // strip the #
    if COMMAND_MESSAGES {
        let cstate = if let Some( cstate ) = state.channels.get_mut( channel ) {
            cstate
        } else {
            println!("parse_command: failed to find channel {}", channel );
            return Err("parse command failed to find channel".into());
        };

        match msg.data() {
            "!fuckoff" => {
                cstate.send_message(runner, "fine, i'll fuck off").await;
                cstate.next_advice = BACKOFF_ADVICE_INTERVAL;
                state.set_mood(channel, Mood::Backoff);
                return Ok(());
            }
            "!comeback" => {
                cstate.send_message(runner, "i knew you would miss me").await;
                cstate.next_advice = PASSIVE_ADVICE_INTERVAL;
                state.set_mood(channel, Mood::Normal);
                return Ok(());
            }
            "!feed" => {
                cstate.send_message(runner, "Mmm i love tendies").await;
                return Ok(());
            }
            "!bot" => {
                cstate.send_message(runner, "github.com/schecko/cynobot").await;
                return Ok(());
            }
            "!mood" => {
                cstate.send_message(runner, &format!("Mr/Ms streamer is feeling {}", cstate.mood)).await;
                return Ok(());
            }
            "!purpose" => {
                cstate.send_message(runner, "my purpose in life is to troll @SomewhatAccurate and his viewers.").await;
                return Ok(());
            }
            "!about" => {
                cstate.send_message(runner, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
                return Ok(());
            }
            "!random" => { 
                send_passive_advice(cstate, lm, runner).await;
                return Ok(());
            }
            "!question" => { 
                send_passive_question(cstate, lm, runner).await;
                return Ok(());
            }
            "!ignoreme" => { 
                let raw_response = "ignoring {user}";
                let result = subst_context(cstate, msg.name(), "", Cow::Borrowed(raw_response));
                state.ignores.insert(msg.name().to_string());
                cstate.send_message(runner, &result).await;
                return Ok(());
            }
            "!noticeme" => { 
                let raw_response = "senpai is noticing you {user}";
                let result = subst_context(cstate, msg.name(), "", Cow::Borrowed(raw_response));
                state.ignores.remove(msg.name());
                cstate.send_message(runner, &result).await;
                return Ok(());
            }
            _ => {}
        }
    }

    if TRIGGER_MESSAGES {
        handle_triggers( state, lm, runner, msg ).await?;
    }

    Ok(())
}

async fn handle_message(state: &mut State<'_>, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, msg: messages::Commands<'_>) {
    use messages::Commands::*;
    match msg {
        Privmsg(msg) => {
            let channel = &msg.channel()[1..]; // strip the #
            println!("[{}] {}: {}", channel, msg.name(), msg.data());
            let _ = parse_command(state, lm, runner, &msg).await.unwrap();
            if let Some( cstate ) = state.channels.get_mut(channel) {
                cstate.dedup_message = false;
            }
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
