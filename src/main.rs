use anyhow::Context as _;
use dotenvy::dotenv;
use poise::serenity_prelude as serenity;
use serenity::{
    Client,
    model::gateway::GatewayIntents,
};
use songbird::{Config, SerenityInit, driver::{DecodeMode, DecodeConfig}};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

mod command;
mod voice;

use command::*;
use voice::SharedRecordingState;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub struct RecordingSession {
    pub guild_id: u64,
    pub session_dir: PathBuf,
    pub state: SharedRecordingState,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub storage_task: Option<JoinHandle<()>>,
}

impl RecordingSession {
    pub fn new(guild_id: u64) -> Self {
        let timestamp = chrono::Utc::now();
        let timestamp_str = timestamp.format("%Y_%m_%d_%H_%M_%S").to_string();
        let session_dir = PathBuf::from("recordings")
            .join(guild_id.to_string())
            .join(&timestamp_str);

        Self {
            guild_id,
            session_dir,
            state: voice::create_recording_session(),
            started_at: timestamp,
            storage_task: None,
        }
    }

    pub fn duration(&self) -> chrono::Duration {
        chrono::Utc::now() - self.started_at
    }
}

type ActiveSessions = HashMap<u64, RecordingSession>;

pub struct Data {
    pub active_sessions: Mutex<ActiveSessions>,
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::Setup { error, .. } => panic!("Failed to start bot: {:?}", error),
        poise::FrameworkError::Command { error, ctx, .. } => {
            tracing::error!("Error in command `{}`: {:?}", ctx.command().name, error);
        }
        error => {
            if let Err(e) = poise::builtins::on_error(error).await {
                tracing::error!("Error while handling error: {}", e);
            }
        }
    }
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let udp_rx_filter = tracing_subscriber::filter::FilterFn::new(|meta| {
        !meta.target().contains("songbird::driver::tasks::udp_rx")
    });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(udp_rx_filter)
        .with(fmt::layer())
        .init();

    std::fs::create_dir_all("recordings").ok();

    let options = poise::FrameworkOptions {
        commands: vec![
            list_voice_users(),
            start_recording(),
            stop_recording(),
            reconstruct_audio(),
        ],
        prefix_options: poise::PrefixFrameworkOptions {
            prefix: Some("/".into()),
            edit_tracker: Some(Arc::new(poise::EditTracker::for_timespan(
                Duration::from_secs(3600),
            ))),
            ..Default::default()
        },
        on_error: |error| Box::pin(on_error(error)),
        pre_command: |ctx| {
            Box::pin(async move {
                info!("Executing command {}", ctx.command().qualified_name);
            })
        },
        post_command: |ctx| {
            Box::pin(async move {
                info!("Executed command {}", ctx.command().qualified_name);
            })
        },
        ..Default::default()
    };

    let token = std::env::var("DISCORD_TOKEN").context("Set DISCORD_TOKEN environment variable")?;

    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                info!("Logged in as {}", _ready.user.name);
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;

                if let Ok(guild_id_str) = std::env::var("GUILD_ID") {
                    if let Ok(guild_id) = guild_id_str.parse::<u64>() {
                        let guild_id = serenity::model::id::GuildId::new(guild_id);
                        poise::builtins::register_in_guild(
                            ctx,
                            &framework.options().commands,
                            guild_id,
                        )
                        .await?;
                        info!("Registered commands for guild {}", guild_id);
                    } else {
                        tracing::error!("Invalid GUILD_ID format: {}", guild_id_str);
                    }
                }

                Ok(Data {
                    active_sessions: Mutex::new(HashMap::new()),
                })
            })
        })
        .options(options)
        .build();

    let songbird_config = Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::default()));

    let mut client = Client::builder(token, intents)
        .framework(framework)
        .register_songbird_from_config(songbird_config)
        .await?;

    client.start().await?;
    Ok(())
}
