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
use std::collections::HashMap;
use std::fs::{ self, File };
use std::io::prelude::*;
use std::path::{ Path, PathBuf };

const TRIGGERS_FILE: &str = "triggers.map";

fn load_list<'a>(contents: &'a str) -> Vec<&'a str> {
    let mut data = Vec::new();
    for line in contents.lines() {
        if line.len() == 0 { continue; }
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
fn load_map<'a>(contents: &'a str, lists: &HashMap<&'a str, Vec<&'a str>>) -> (Vec<MultiTrigger<'a>>, HashMap<&'a str, MapValue<'a>>) {
    let mut map = HashMap::new();
    let mut multi_triggers = Vec::new();
    for line in contents.lines() {
        let mut split = line.split('='); 
        if let (Some(meta_key), Some(value)) = (split.next(), split.next()) {
            if meta_key.len() == 0 { continue; }
            if value.len() == 0 { continue; }

            let single = vec![meta_key];
            let keys = if let Some('[') = meta_key.chars().next() {
                lists.get(&meta_key[1..]).unwrap()
            } else {
                &single
            };

            'key_loop: for key in keys { 
                if key.contains('{') { continue 'key_loop; }

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
                    map.insert(*key, map_value);
                }
            }
        }
    } 
    (multi_triggers, map)
}

const ADVICE: &[&str] = &[
    "Don't forget to commit, feeder.",
    "Bro you should take a break.",
    "Eat tendies frequently.",
];

const PASSIVE_MESSAGES: bool = true;
const TRIGGER_MESSAGES: bool = true;
const COMMAND_MESSAGES: bool = true;

async fn connect(user_config: &UserConfig, channel: &str) -> anyhow::Result<AsyncRunner> {
    let connector = connector::tokio::ConnectorRustTls::twitch()?;

    println!("Connecting...");
    let mut runner = AsyncRunner::connect(connector, user_config).await?;
    println!("..Connected, attempting to join channel '{}'", channel);
    let _ = runner.join(&channel).await?;
    println!("joined '{}'!", channel);

    Ok(runner)
}

fn data_dir() -> anyhow::Result<PathBuf> {
    Ok(std::env::current_dir()?.join("data"))
}

fn load_file_rel(name: &str) -> anyhow::Result<String> { 
    let full_path = data_dir()?.join(name);
    load_file(&full_path)
}

fn load_file(full_path: &Path) -> anyhow::Result<String> {
    println!("path {:?}", full_path);
    let mut file = File::open(full_path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (user_config, channel) = get_config()?;

    let runner = connect(&user_config, &channel).await?;
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
        map.insert(&content.0[..], load_list(&content.1)); 
    }

    let triggers_content = load_file_rel(TRIGGERS_FILE)?;
    let (multi_triggers, triggers) = load_map(&triggers_content, &map); 

    println!("lists {:#?}", map);
    println!("multi triggers {:#?}", multi_triggers);
    println!("triggers {:#?}", triggers);

    let state = State::new(channel, triggers, multi_triggers, map);

    main_loop(state, runner).await
}

fn get_env_var(key: &str) -> anyhow::Result<String> {
    std::env::var(key).with_context(|| format!("please set `{}`", key))
}

pub fn get_config() -> anyhow::Result<(twitchchat::UserConfig, String)> {
    let name = get_env_var("TWITCH_NAME")?;
    let token = get_env_var("TWITCH_TOKEN")?;
    let channel = get_env_var("TWITCH_CHANNEL")?;

    let config = UserConfig::builder()
        // twitch account name
        .name(name)
        // OAuth token
        .token(token)
        .enable_all_capabilities()
        .build()?;

    Ok((config, channel))
}

const PASSIVE_ADVICE_INTERVAL: Duration = Duration::from_secs(60 * 30); // 30min
const BACKOFF_ADVICE_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24); // 24h

#[derive(Display)]
pub enum Mood {
    #[strum(to_string = "normal")]
    Normal,
    #[strum(to_string = "agitated")]
    Backoff,
}

pub struct State<'a> {
    pub channel: String,
    pub dedup_message: bool,
    pub last_advice: Instant,
    pub lists: HashMap<&'a str, Vec<&'a str>>,
    pub mood: Mood,
    pub multi_triggers: Vec<MultiTrigger<'a>>,
    pub next_advice: Duration,
    pub triggers: HashMap<&'a str, MapValue<'a>>,
}

impl<'a> State<'a> {
    fn new(
        channel: String, 
        triggers: HashMap<&'a str, MapValue<'a>>, 
        multi_triggers: Vec<MultiTrigger<'a>>, 
        lists: HashMap<&'a str, Vec<&'a str>>
    ) -> Self {
        State {
            channel,
            dedup_message: false,
            last_advice: Instant::now(),
            lists,
            mood: Mood::Normal,
            multi_triggers,
            next_advice: PASSIVE_ADVICE_INTERVAL,
            triggers,
        }
    }

    fn set_mood(&mut self, mood: Mood)
    {
        self.mood = mood;
    }

    async fn send_message(&mut self, runner: &mut AsyncRunner, msg: &str) {
        let mut writer = runner.writer();
        let cmd = commands::privmsg(&self.channel, msg);
        writer.encode(cmd).await.unwrap();

        self.dedup_message = true;
        self.last_advice = Instant::now();
    }
}

pub async fn main_loop(mut state: State<'_>, mut runner: AsyncRunner) -> anyhow::Result<()> {
    loop {
        match runner.next_message().await? {
            // this is the parsed message -- across all channels (and notifications from Twitch)
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

        if state.last_advice + state.next_advice < Instant::now() {
            match state.mood {
                Mood::Normal => {
                    let mut rng = rand::thread_rng();
                    let msg = ADVICE[rng.gen::<usize>() % ADVICE.len()];
                    if PASSIVE_MESSAGES && !state.dedup_message {
                        state.send_message(&mut runner, msg).await
                    }
                }
                Mood::Backoff => {
                    state.set_mood(Mood::Normal);
                    state.next_advice = PASSIVE_ADVICE_INTERVAL;
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

fn substitute_random(state: &State<'_>, message: &str) -> String { 
    println!("substituting {}", message);
    let mut result = String::from(message);
    for substitution in SubLocations::new(message) {
        println!("found substitution location {}", substitution);
        if substitution.len() < 3 { continue; }
        if let Some(list) = state.lists.get(&substitution[1..substitution.len() - 1]) {
            let mut rng = rand::thread_rng();
            let msg = list[rng.gen::<usize>() % list.len()];
            println!("substituting {} for {}", substitution, msg);
            result = result.replace(substitution, msg); 
            println!("intermediate sub {}", result);
        }
    }
    result 
}

fn make_response(state: &State<'_>, trigger: &str, map_value: &MapValue<'_>) -> Option<String> {
    match map_value{
        MapValue::FileName(name) => {
            if let Some(list) = state.lists.get(*name) {
                let mut rng = rand::thread_rng();
                let msg = list[rng.gen::<usize>() % list.len()];
                println!("detected file {}", name);
                let mut result = substitute_random(state, msg);
                result = result.replace("{trigger}", trigger);
                return Some(result);
            }
        }
        MapValue::Value(value) => {
            println!("detected value {}", value);
            let mut result = substitute_random(state, value);
            result = result.replace("{trigger}", trigger);
            return Some(result);
        }
    } 
    return None;
}

async fn parse_command(state: &mut State<'_>, runner: &mut AsyncRunner, msg: &messages::Privmsg<'_>) -> anyhow::Result<()> {
    if COMMAND_MESSAGES {
        match msg.data() {
            "fuckoff" | "fuck off" => {
                state.send_message(runner, "No, you fuck off.").await;
                return Ok(());
            }
            "!fuckoff" => {
                state.send_message(runner, "Fine, I'll fuck off.").await;
                state.next_advice = BACKOFF_ADVICE_INTERVAL;
                state.set_mood(Mood::Backoff);
                return Ok(());
            }
            "!feed" => {
                state.send_message(runner, "Mmm I love tendies.").await;
                return Ok(());
            }
            "!bot" => {
                state.send_message(runner, "github.com/schecko/cynobot").await;
                return Ok(());
            }
            "!mood" => {
                state.send_message(runner, &format!("Mr/Ms streamer is feeling {}.", state.mood)).await;
                return Ok(());
            }
            "!purpose" => {
                state.send_message(runner, "My purpose in life is to troll @SomewhatAccurate and his viewers.").await;
                return Ok(());
            }
            "!about" => {
                state.send_message(runner, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
                return Ok(());
            }
            _ => {}
        }
    }

    if TRIGGER_MESSAGES { 
        println!("triggers {:#?}", state.triggers);
        println!("lists {:#?}", state.lists);
        println!("multi triggers {:#?}", state.multi_triggers);
        let lower_case = msg.data().to_lowercase();
        // todo ignore punctuation?
        for token in lower_case.split_whitespace() {
            match state.triggers.get(token) {
                Some(value) => {
                    if let Some(response) = make_response(state, token, value) {
                        state.send_message(runner, &response).await; 
                    }
                }
                _ => {}
            }
        }

        let mut opt_response = None;
        'outer: for multi_trigger in &state.multi_triggers {
            let mut found = false;
            'inner: for trigger in &multi_trigger.triggers {
                if *trigger == "" { 
                    if found { 
                        break 'inner;
                    } else {
                        continue 'outer; 
                    }
                } 

                if lower_case.contains(trigger) {
                    found = true; 
                } else {
                    break 'inner;
                }
            }

            if found { 
                opt_response = make_response(state, multi_trigger.triggers[0], &multi_trigger.value);
            }
        }
        if let Some(response) = opt_response {
            state.send_message(runner, &response).await; 
        } 
    }

    Ok(())
}

async fn handle_message(state: &mut State<'_>, runner: &mut AsyncRunner, msg: messages::Commands<'_>) {
    use messages::Commands::*;
    match msg {
        Privmsg(msg) => {
            println!("[{}] {}: {}", msg.channel(), msg.name(), msg.data());
            let _ = parse_command(state, runner, &msg).await.unwrap();
            state.dedup_message = false;
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
