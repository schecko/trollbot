// TODO 
// - per trigger cooldowns

use crate::config::*;
use crate::file_help::*;
use crate::list::*;
use crate::map::*;

use twitchchat::{
    commands, connector, messages,
    runner::{AsyncRunner, Status},
    UserConfig,
};
use itertools::Itertools;
use rand::Rng;
use serde::{ Serialize, Deserialize };
use std::borrow::Borrow;
use std::borrow::Cow;
use std::collections::{ HashMap, HashSet };
use std::error::Error;
use std::fs::File;
use std::hash::{ Hash, Hasher };
use std::io::prelude::*;
use std::time::{ Duration, SystemTime };
use strum::*;

const PASSIVE_ADVICE_INTERVAL: Duration = Duration::from_secs(60 * 60 * 3); // 3h
const BACKOFF_ADVICE_INTERVAL: Duration = Duration::from_secs(60 * 60 * 24); // 24h

const PASSIVE_MESSAGE_RANGE: MinMax::<Duration> = MinMax::<Duration>::new( Duration::from_secs(1200), Duration::from_secs(1800) ); 

const STATE_SAVE_INTERVAL: Duration = Duration::from_secs(60);

const TRIGGERS_FILE: &str = "triggers.map";
const CONFIG_CHANNELS: &str = "channels.list";
const CONFIG_COMMANDS: &str = "commands.map";
const COMMANDS_TEXT_FILE: &str = "commands_text.map";

const PASSIVE_MESSAGES: bool = true;
const TRIGGER_MESSAGES: bool = true;
const COMMAND_MESSAGES: bool = true;

#[derive(Deserialize, Serialize)]
pub struct MinMax<T> {
    min: T,
    max: T,
}

impl<T> MinMax<T> { 
    pub const fn new(min: T, max: T) -> Self {
        MinMax {
            min,
            max
        }
    }
}

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

async fn connect_run() -> Result<(), Box<dyn Error>> {
    let channels_content = load_config_file(CONFIG_CHANNELS)?;
    let (user_config, channels) = get_config(&channels_content)?;

    let runner = connect(&user_config, &channels).await?;
    println!("starting main loop"); 

    let dir = std::fs::read_dir(data_dir()?)?;
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

    // map a command to some text the user sees
    let command_text_content = load_file_rel(COMMANDS_TEXT_FILE)?;
    let (_, commands_text) = load_map(&command_text_content, &map); 

    // map a command to a code operation
    let command_content = load_config_file(CONFIG_COMMANDS)?;
    let (_, commands) = load_map(&command_content, &map); 

    //println!("lists {:#?}", map);
    //println!("multi triggers {:#?}", multi_triggers);
    //println!("triggers {:#?}", triggers);
    //println!("commands {:#?}", commands);
    //println!("commands text {:#?}", commands_text);

    let meta_state = MetaState::new();
    let state = MetaState::try_read_state(channels);
    let lm = ListsMaps::new( commands, commands_text, map, multi_triggers, triggers);

    main_loop(meta_state, state, &lm, runner).await 
}

#[derive(Display, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mood {
    #[strum(to_string = "normal")]
    Normal,
    #[strum(to_string = "agitated")]
    Backoff,
}

#[derive(Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_name: String,
    pub dedup_message: bool, 
    pub direct_message: bool,
    pub last_advice: SystemTime,
    pub last_message: SystemTime,
    pub mood: Mood,
    pub next_advice: Duration,
    pub next_message: MinMax<Duration>,
    pub off_topic: Option<SystemTime>,
    pub current_topic: Option<String>,
    pub total_off_topic: Duration,
}

impl ChannelState { 
    fn new(channel_name: &str) -> Self {
        Self { 
            direct_message: false,
            channel_name: String::from(channel_name),
            dedup_message: false,
            last_advice: SystemTime::now(), 
            last_message: SystemTime::now(),
            mood: Mood::Normal, 
            next_advice: PASSIVE_ADVICE_INTERVAL, 
            next_message: PASSIVE_MESSAGE_RANGE,
            off_topic: None,
            current_topic: None,
            total_off_topic: Duration::new(0, 0),
        } 
    }

    fn set_mood(&mut self, mood: Mood) {
        self.mood = mood;
    }

    async fn send_message(&mut self, runner: &mut AsyncRunner, msg: &str) {
        let mut rng = rand::thread_rng();
        let next_message = rng.gen_range(self.next_message.min..self.next_message.max);
        if self.direct_message || self.last_message + next_message < SystemTime::now() {
            self.force_send_message(runner, msg).await;
        }
    }

    async fn force_send_message(&mut self, runner: &mut AsyncRunner, msg: &str) {
        println!("sending message [{}]", msg);
        let mut writer = runner.writer();
        let cmd = commands::privmsg(&self.channel_name, msg);
        writer.encode(cmd).await.unwrap();

        self.dedup_message = true;
        self.last_advice = SystemTime::now();
        self.last_message = SystemTime::now();
        self.direct_message = false;
    }
}

pub struct ListsMaps<'a>
{
    pub commands: HashMap<Cow<'a, str>, MapValue<'a>>,
    pub command_text: HashMap<Cow<'a, str>, MapValue<'a>>,
    pub lists: HashMap<&'a str, Vec<&'a str>>,
    pub multi_triggers: Vec<MultiTrigger<'a>>,
    pub triggers: HashMap<Cow<'a, str>, MapValue<'a>>,
}

impl<'a> ListsMaps<'a>
{
    fn new(
        commands: HashMap<Cow<'a, str>, MapValue<'a>>,
        command_text: HashMap<Cow<'a, str>, MapValue<'a>>,
        lists: HashMap<&'a str, Vec<&'a str>>,
        multi_triggers: Vec<MultiTrigger<'a>>, 
        triggers: HashMap<Cow<'a, str>, MapValue<'a>>, 
    ) -> Self {
        ListsMaps {
            commands,
            command_text,
            lists,
            multi_triggers,
            triggers,
        } 
    }
}

pub struct MetaState
{
    pub next_state_save: SystemTime,
}

impl MetaState
{
    fn new() -> Self
    {
        MetaState {
            next_state_save: SystemTime::now() + STATE_SAVE_INTERVAL,
        }
    }

    fn clean_temp_files() -> anyhow::Result<()>
    {
        for dir_entry in std::fs::read_dir(user_dir()?)? {
            let path = dir_entry?.path();
            let temp_ext = std::ffi::OsStr::new("temp");
            if path.extension().filter(|ext| ext == &temp_ext).is_some() {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn try_write_state(&mut self, state: &State) -> anyhow::Result<()>
    {
        if self.next_state_save < SystemTime::now() {
            let mut temp_file = user_dir()?;
            temp_file.push(format!("{}-state.json.temp", rand::thread_rng().gen::<u32>()));
            let serialized = serde_json::to_string(state);
            std::fs::create_dir_all(&temp_file.parent().unwrap()).unwrap();
            let mut file = File::options()
                .write(true)
                .create(true)
                .open(&temp_file)
                .unwrap();

            file.write_all(serialized.unwrap().as_bytes())?;

            let mut state_file_name = user_dir()?;
            state_file_name.push("state.json");
            std::fs::create_dir_all(state_file_name.parent().unwrap()).unwrap();
            std::fs::rename(temp_file, state_file_name)?;
            self.next_state_save = SystemTime::now() + STATE_SAVE_INTERVAL;
        }
        Ok(())
    }

    fn try_read_state<'a>(channels: Vec<&'a str>) -> State
    {
        let mut state_file_name = user_dir().unwrap();
        state_file_name.push("state.json");

        let contents = match load_file(&state_file_name) {
            Ok(contents) => contents,
            Err(_err) => return State::new(channels),
        };
        
        // assume we want to crash if file exists but deserialization fails
        State::merge(channels, serde_json::from_str(&contents).unwrap())
    }
}

#[derive(Deserialize, Serialize)]
pub struct State {
    // TODO: improve. hash is from channel name... just don't want to allocate every query...
    pub channels: HashMap<u64, ChannelState>,
    pub ignores: HashSet<String>,
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
            ignores: HashSet::new(),
        }
    }

    fn chash(channel_name: &str) -> u64 {
        // TODO ensure the hash is stable for io
        let mut s = std::collections::hash_map::DefaultHasher::new();
        channel_name.hash(&mut s);
        s.finish()
    }

    fn merge(channels: Vec<&str>, mut state: State) -> State
    {
        for channel in channels {
            state.channels.entry(Self::chash(channel)).or_insert_with(|| ChannelState::new(channel));
        }
        state
    }

    fn set_mood(&mut self, channel: &str, mood: Mood) {
        if let Some( channel_state ) = self.channels.get_mut(&State::chash(channel)) {
            channel_state.mood = mood;
        }
    } 
}

async fn send_passive_advice(state: &mut ChannelState, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, force: bool) {
    let passive = lm.lists.get("passive_advice").unwrap();
    let mut rng = rand::thread_rng();
    let msg = passive[rng.gen::<usize>() % passive.len()]; 
    let result = substitute_random(lm, msg); 
    if force {
        state.force_send_message(runner, &result).await
    } else {
        state.send_message(runner, &result).await
    }
}

async fn send_passive_question(state: &mut ChannelState, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, force: bool) {
    let passive = lm.lists.get("questions").unwrap();
    let mut rng = rand::thread_rng();
    let msg = passive[rng.gen::<usize>() % passive.len()]; 
    let result = substitute_random(lm, msg); 
    if force {
        state.force_send_message(runner, &result).await
    } else {
        state.send_message(runner, &result).await
    }
}

pub async fn main_loop(mut meta_state: MetaState, mut state: State, lm: &ListsMaps<'_>, mut runner: AsyncRunner) -> Result<(), Box<dyn Error>> {
    loop {
        MetaState::clean_temp_files()?;
        meta_state.try_write_state(&state)?;

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
            if cstate.last_advice + cstate.next_advice < SystemTime::now() {
                match cstate.mood {
                    Mood::Normal => {
                        if PASSIVE_MESSAGES && !cstate.dedup_message { 
                            send_passive_advice(cstate, lm, &mut runner, false).await;
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

struct ReplyContext<'a>
{
    user: &'a str,
    trigger: &'a str,
    trigger_message: &'a str,
}

fn subst_context<'a>(state: &ChannelState, context: &ReplyContext<'_>, message: Cow<'a, str>) -> Cow<'a, str> { 
    if message.contains("{") {
        let mut result = message.replace("{trigger}", context.trigger);
        result = result.replace("{trigger_message}", context.trigger_message);
        result = result.replace("{user}", context.user);
        result = result.replace("{channel}", &state.channel_name);
        return subst_global(Cow::Owned(result));
    } else {
        return message;
    } 
}

fn make_response<'a>(state: &ChannelState, lm: &ListsMaps<'a>, context: &ReplyContext<'_>, map_value: &MapValue<'a>) -> Option<Cow<'a, str>> {
    match map_value {
        MapValue::FileName(name) => {
            if let Some(list) = lm.lists.get(*name) {
                let mut rng = rand::thread_rng();
                let msg = list[rng.gen::<usize>() % list.len()];
                println!("detected file {}", name);
                let mut result = substitute_random(lm, msg);
                result = subst_context(state, context, result);
                return Some(result);
            }
        }
        MapValue::Value(value) => {
            println!("detected value {}", value);
            let mut result = substitute_random(lm, value);
            result = subst_context(state, context, result);
            return Some(result);
        }
    } 
    return None;
}

fn make_response_message<'b>(state: &ChannelState, lm: &ListsMaps<'b>, context: &ReplyContext<'_>, msg: &'b str) -> Cow<'b, str> {
    let result = substitute_random(lm, msg);
    return subst_context(state, context, result);
}

async fn handle_triggers(state: &mut State, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, msg: &messages::Privmsg<'_>) -> anyhow::Result<()> {
    let channel = &msg.channel()[1..]; // strip the #
    if let Some( cstate ) = state.channels.get_mut(&State::chash(channel)) {
        if cstate.mood == Mood::Normal && !state.ignores.contains(msg.name()) { 
            let lower_case = msg.data().to_lowercase();
            // todo ignore punctuation?
            for token in lower_case.split_whitespace() {
                match lm.triggers.get(token) {
                    Some(value) => {
                        let context = ReplyContext
                        {
                            user: msg.name(),
                            trigger: token,
                            trigger_message: msg.data(),
                        };
                        if let Some(response) = make_response(cstate, lm, &context, value) {
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

                    let context = ReplyContext
                    {
                        user: msg.name(),
                        trigger: "",
                        trigger_message: msg.data(),
                    };

                    let trigger_subst = subst_context(cstate, &context, Cow::Borrowed(trigger)); 
                    if lower_case.contains(&*trigger_subst) {
                        found = true; 
                    } else {
                        found = false;
                        break 'inner;
                    }
                }

                if found { 
                    let trigger = multi_trigger.triggers.join(" ");
                    let context = ReplyContext
                    {
                        user: msg.name(),
                        trigger: &trigger,
                        trigger_message: msg.data(),
                    };
                    opt_response = make_response(cstate, lm, &context, &multi_trigger.value);
                }
            }
            if let Some(response) = opt_response {
                cstate.send_message(runner, &response).await; 
            } 
        }
    }
    Ok(())
}

async fn parse_command(state: &mut State, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, msg: &messages::Privmsg<'_>) -> Result<(), Box<dyn Error>> {
    let channel = &msg.channel()[1..]; // strip the #
    if COMMAND_MESSAGES {
        let cstate = if let Some( cstate ) = state.channels.get_mut(&State::chash(channel)) {
            cstate
        } else {
            println!("parse_command: failed to find channel {}", channel );
            return Err("parse command failed to find channel".into());
        }; 

        let mut was_command = false;
        if let Some(MapValue::Value(command_text)) = lm.command_text.get(msg.data()) {
            println!("got command {}", command_text);
            let context = ReplyContext
            {
                user: msg.name(),
                trigger: msg.data(),
                trigger_message: msg.data(),
            };
            let result = subst_context(cstate, &context, Cow::Borrowed(command_text));

            cstate.force_send_message(runner, &result).await;
            was_command = true;
        }

        let mut commands = msg.data().split_whitespace();

        if let Some(MapValue::Value(command)) = lm.commands.get(commands.next().unwrap_or("")) {
            let context = ReplyContext
            {
                user: msg.name(),
                trigger: msg.data(),
                trigger_message: msg.data(),
            };

            let mut map_command_split = command.split_whitespace();

            match map_command_split.next().unwrap_or("") {
                "COMMANDS" => {
                    let keys: HashSet<&str> = lm.command_text.keys().chain( lm.commands.keys() ).map(|k| k.borrow()).collect();
                    let msg: String = keys.iter().join(", ");
                    cstate.force_send_message(runner, &msg).await; 
                    return Ok(());
                }
                "CONFIG" => {
                    let lower_case = msg.data().to_lowercase();
                    let mut iter = lower_case.split_whitespace();
                    iter.next(); // ignore the command, which would be the substituted "CONFIG"
                    
                    match iter.next() {
                        Some("cd") => {
                            let error_msg = "invalid command, expected format \"cd <min> <max>\" where <min> and <max> are integer numbers";
                            if let (Some(min_str), Some(max_str)) = (iter.next(), iter.next()) {
                                if let (Ok(a), Ok(b)) = (min_str.parse::<u64>(), max_str.parse::<u64>()) {
                                    let min = if a > b { b } else { a };
                                    let max = if a > b { a } else { b }; 
                                    cstate.next_message = MinMax::new(Duration::from_secs(min), Duration::from_secs(max));
                                    cstate.force_send_message(runner, &format!("successfully changed message cooldown to {}s-{}s", min, max)).await;
                                } else {
                                    cstate.force_send_message(runner, error_msg).await;
                                }
                            } else {
                                cstate.force_send_message(runner, error_msg).await;
                            }
                        }
                        _ => {
                            println!("detected unknown CONFIG subcommand in '{}'", msg.data());
                        } 
                    } 
                    
                    return Ok(());
                }
                "LEAVE" => {
                    cstate.next_advice = BACKOFF_ADVICE_INTERVAL;
                    state.set_mood(channel, Mood::Backoff);
                    return Ok(());
                }
                "JOIN" => {
                    cstate.next_advice = PASSIVE_ADVICE_INTERVAL;
                    state.set_mood(channel, Mood::Normal);
                    return Ok(());
                }
                "RANDOM_STATEMENT" => { 
                    send_passive_advice(cstate, lm, runner, true).await;
                    return Ok(());
                }
                "RANDOM_QUESTION" => { 
                    send_passive_question(cstate, lm, runner, true).await;
                    return Ok(());
                }
                "IGNORE_ME" => { 
                    state.ignores.insert(msg.name().to_string());
                    return Ok(());
                }
                "NOTICE_ME" => { 
                    state.ignores.remove(msg.name());
                    return Ok(());
                }
                "OFF_TOPIC" => { 
                    let response = match &cstate.off_topic {
                        Some(stamp) => {
                            let duration = SystemTime::now().duration_since(*stamp).unwrap();
                            format!("{channel} has already been off topic for {}h {}m {}s",
                                    duration.as_secs() / 60 / 60,
                                    duration.as_secs() / 60 % 60,
                                    duration.as_secs() % 60 )
                        }
                        None => {
                            cstate.off_topic = Some(SystemTime::now());
                            format!("starting off topic timer")
                        }
                    };
                    cstate.force_send_message(runner, &make_response_message(&cstate, &lm, &context, &response)).await;
                    return Ok(());
                }
                "ON_TOPIC" => { 
                    if let Some(start) = cstate.off_topic.take() {
                        let duration = SystemTime::now().duration_since(start).unwrap();
                        cstate.total_off_topic += duration;
                        let response = format!("{channel} is finally on topic, it took them {}h {}m {}s", duration.as_secs() / 60 / 60, duration.as_secs() / 60 % 60, duration.as_secs() % 60 );
                        cstate.force_send_message(runner, &make_response_message(&cstate, &lm, &context, &response)).await;
                    }
                    return Ok(());
                }
                "TOTAL_OFF_TOPIC" => { 
                    let response = format!("The streamer has been off topic a total of {}h {}m {}s", cstate.total_off_topic.as_secs() / 60 / 60, cstate.total_off_topic.as_secs() / 60 % 60, cstate.total_off_topic.as_secs() % 60 );
                    cstate.force_send_message(runner, &make_response_message(&cstate, &lm, &context, &response)).await;
                    return Ok(());
                }
                "SET_TOPIC" => { 
                    let topic = commands.next().unwrap_or("");
                    let response = format!("current topic is now {}", topic);
                    cstate.force_send_message(runner, &make_response_message(&cstate, &lm, &context, &response)).await;
                    return Ok(());
                }
                "RESETCD" => { 
                    cstate.last_message = SystemTime::UNIX_EPOCH;
                    return Ok(());
                }
                "REPEAT_MAPPED" => { 
                    // TODO get the space from the input command, but its mangled from
                    // split_whitespace
                    let text_to_repeat = format!("{} ", map_command_split.join(" "));
                    let repeat_count = std::cmp::min(commands.next().unwrap_or("").parse::<i32>().unwrap_or(1), 200);
                    let repeated = (0..repeat_count).map(|_| &text_to_repeat[..]).collect::<String>();
                    cstate.force_send_message(runner, &make_response_message(&cstate, &lm, &context, &repeated)).await;
                    return Ok(());
                }
                "REPEAT" => { 
                    let repeat_count = std::cmp::min(commands.next().unwrap_or("").parse::<i32>().unwrap_or(1), 200);
                    // TODO get the space from the input command, but its mangled from
                    // split_whitespace
                    let text_to_repeat = format!("{} ", commands.join(" "));
                    let repeated = (0..repeat_count).map(|_| &text_to_repeat[..]).collect::<String>();
                    cstate.force_send_message(runner, &make_response_message(&cstate, &lm, &context, &repeated)).await;
                    return Ok(());
                }
                _ => {}
            }
        }

        if was_command {
            return Ok(());
        }
    }


    if TRIGGER_MESSAGES {
        handle_triggers( state, lm, runner, msg ).await?;
    }

    Ok(())
}

async fn handle_message(state: &mut State, lm: &ListsMaps<'_>, runner: &mut AsyncRunner, msg: messages::Commands<'_>) {
    use messages::Commands::*;
    match msg {
        Privmsg(msg) => {
            let channel = &msg.channel()[1..]; // strip the #
            println!("[{}] {}: {}", channel, msg.name(), msg.data());
            let _ = parse_command(state, lm, runner, &msg).await.unwrap();
            if let Some( cstate ) = state.channels.get_mut(&State::chash(channel)) {
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

pub async fn main() -> !
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
        println!("disconnected, reconnecting in {:?}s", sleep_duration);
        std::thread::sleep(sleep_duration);
        last_start_time = start_time;
    }
}

