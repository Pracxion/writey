pub mod export_audio;
pub mod list_voice_users;
pub mod reconstruct_audio;
pub mod start_recording;
pub mod stop_recording;

pub use export_audio::export_audio;
pub use list_voice_users::list_voice_users;
pub use reconstruct_audio::reconstruct_audio;
pub use start_recording::start_recording;
pub use stop_recording::stop_recording;

use crate::Context;
use crate::Error;

/// Check used by recording commands. If `AUTHORIZED_USER_ID` is set, only that
/// Discord user may proceed; if it is unset or empty, the command is open to all.
pub async fn is_authorized(ctx: Context<'_>) -> Result<bool, Error> {
    let allowed = match std::env::var("AUTHORIZED_USER_ID") {
        Ok(v) => v,
        Err(_) => return Ok(true),
    };
    let allowed = allowed.trim();
    if allowed.is_empty() {
        return Ok(true);
    }
    if allowed.parse::<u64>().ok() == Some(ctx.author().id.get()) {
        return Ok(true);
    }
    ctx.say("You are not authorized to use this command.")
        .await?;
    Ok(false)
}
