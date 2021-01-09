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

const TRIGGERS: &[&str] = &[
    ".net",
    "abap",
    "action!",
    "actionscript",
    "ada",
    "ada",
    "adenine",
    "adga",
    "advpl",
    "agilent",
    "agora",
    "aimms",
    "aldor",
    "c",
    "c#",
    "c++",
    "clojure",
    "cobol",
    "cobol",
    "cpp",
    "dart",
    "delphi",
    "fortran",
    "go",
    "groovy",
    "haskell",
    "jai",
    "java",
    "javascript",
    "julia",
    "kotlin",
    "lisp",
    "lua",
    "matlab",
    "objectivec",
    "pascal",
    "perl",
    "php",
    "powershell",
    "prolog",
    "python",
    "ruby",
    "rust",
    "scala",
    "scratch",
    "swift",
    "typescript",
    "vba",
    "visual basic",
];

const RESPONSES: &[&str] = &[
    "Rust is bae.",
    "You should have chosen Rust instead.",
];

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (user_config, channel) = get_config()?;

    let runner = connect(&user_config, &channel).await?;
    println!("starting main loop");
    let state = State::new(channel);
    main_loop(state, runner).await
}

// some helpers for the demo
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

pub struct State {
    pub channel: String,
    pub mood: Mood,
    pub last_advice: Instant,
    pub next_advice: Duration,
    pub dedup_message: bool,
}

impl State {
    fn new(channel: String) -> Self {
        State {
            channel,
            mood: Mood::Normal,
            last_advice: Instant::now(),
            next_advice: PASSIVE_ADVICE_INTERVAL,
            dedup_message: false,
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

pub async fn main_loop(mut state: State, mut runner: AsyncRunner) -> anyhow::Result<()> {
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

async fn parse_command(state: &mut State, runner: &mut AsyncRunner, msg: &messages::Privmsg<'_>) -> anyhow::Result<()> {
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
        println!("scanning triggers");
        let lower_case = msg.data().to_lowercase();
        for trigger in TRIGGERS {
            if lower_case.contains(trigger) {
                println!("found trigger");
                let mut rng = rand::thread_rng();
                let msg = RESPONSES[rng.gen::<usize>() % RESPONSES.len()];
                state.send_message(runner, msg).await;
                break;
            }
        }
    }

    Ok(())
}

async fn handle_message(state: &mut State, runner: &mut AsyncRunner, msg: messages::Commands<'_>) {
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
