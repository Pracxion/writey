use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::Path;

pub type DbPool = SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserSetting {
    pub user_id: String,
    pub guild_id: String,
    pub transcribe_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn init_db(database_url: &str) -> Result<DbPool, sqlx::Error> {
    if let Some(path) = database_url.strip_prefix("sqlite:") {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

pub async fn get_user_setting(
    pool: &DbPool,
    user_id: &str,
    guild_id: &str,
) -> Result<Option<UserSetting>, sqlx::Error> {
    let setting = sqlx::query_as::<_, UserSetting>(
        "SELECT * FROM user_settings WHERE user_id = ? AND guild_id = ?"
    )
    .bind(user_id)
    .bind(guild_id)
    .fetch_optional(pool)
    .await?;

    Ok(setting)
}

pub async fn set_transcribe_name(
    pool: &DbPool,
    user_id: &str,
    guild_id: &str,
    transcribe_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO user_settings (user_id, guild_id, transcribe_name, updated_at)
        VALUES (?, ?, ?, datetime('now'))
        ON CONFLICT(user_id, guild_id) 
        DO UPDATE SET transcribe_name = excluded.transcribe_name, updated_at = datetime('now')
        "#
    )
    .bind(user_id)
    .bind(guild_id)
    .bind(transcribe_name)
    .execute(pool)
    .await?;

    Ok(())
}

