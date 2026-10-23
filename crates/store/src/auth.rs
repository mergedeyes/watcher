//! Users, sessions and API tokens. Secrets never reach this module in
//! plain text: callers pass the password hash and SHA-256 hashes of tokens.

use crate::{Error, Result, Store, from_epoch};
use serde::Serialize;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct User {
    pub id: i32,
    pub username: String,
    pub admin: bool,
    pub disabled: bool,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub created: SystemTime,
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub last_login: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiToken {
    pub id: i32,
    pub name: String,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub created: SystemTime,
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub last_used: Option<SystemTime>,
}

type UserRow = (i32, String, bool, bool, f64, Option<f64>);
/// A user row plus the password hash.
type LoginRow = (i32, String, bool, bool, f64, Option<f64>, String);

const USER_COLUMNS: &str = "u.id, u.username, u.admin, u.disabled,
    extract(epoch FROM u.created)::float8, extract(epoch FROM u.last_login)::float8";

fn user((id, username, admin, disabled, created, last_login): UserRow) -> User {
    User {
        id,
        username,
        admin,
        disabled,
        created: from_epoch(created),
        last_login: last_login.map(from_epoch),
    }
}

/// Maps a unique violation to [`Error::Conflict`].
pub(crate) fn conflict(err: sqlx::Error, what: impl FnOnce() -> String) -> Error {
    match &err {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
            Error::Conflict(what())
        }
        _ => Error::Db(err),
    }
}

impl Store {
    pub async fn user_count(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM users WHERE NOT disabled")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn users(&self) -> Result<Vec<User>> {
        let sql = format!("SELECT {USER_COLUMNS} FROM users u ORDER BY lower(u.username)");
        let rows: Vec<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(user).collect())
    }

    pub async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        admin: bool,
    ) -> Result<User> {
        let sql = format!(
            "INSERT INTO users AS u (username, password_hash, admin) VALUES ($1, $2, $3)
             RETURNING {USER_COLUMNS}"
        );
        let row: UserRow = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(username)
            .bind(password_hash)
            .bind(admin)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| conflict(e, || format!("user {username:?} already exists")))?;
        Ok(user(row))
    }

    /// Sets a new password and ends all sessions of the user. `false` if
    /// there is no such user.
    pub async fn set_password(&self, username: &str, password_hash: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let id: Option<i32> = sqlx::query_scalar(
            "UPDATE users SET password_hash = $2 WHERE lower(username) = lower($1) RETURNING id",
        )
        .bind(username)
        .bind(password_hash)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(id) = id {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(id.is_some())
    }

    /// Changes the admin and disabled flags; disabling ends the user's
    /// sessions. `None` if there is no such user.
    pub async fn update_user(
        &self,
        username: &str,
        admin: Option<bool>,
        disabled: Option<bool>,
    ) -> Result<Option<User>> {
        let mut tx = self.pool.begin().await?;
        let sql = format!(
            "UPDATE users AS u SET admin = coalesce($2, u.admin), disabled = coalesce($3, u.disabled)
             WHERE lower(u.username) = lower($1) RETURNING {USER_COLUMNS}"
        );
        let row: Option<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(username)
            .bind(admin)
            .bind(disabled)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(row) = &row
            && row.3
        {
            sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                .bind(row.0)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(row.map(user))
    }

    /// Deletes a user with their sessions and tokens.
    pub async fn delete_user(&self, username: &str) -> Result<bool> {
        let n = sqlx::query("DELETE FROM users WHERE lower(username) = lower($1)")
            .bind(username)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(n > 0)
    }

    /// The user and their password hash, for checking a login.
    pub async fn login_user(&self, username: &str) -> Result<Option<(User, String)>> {
        let sql = format!(
            "SELECT {USER_COLUMNS}, u.password_hash FROM users u WHERE lower(u.username) = lower($1)"
        );
        let row: Option<LoginRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(a, b, c, d, e, f, hash)| (user((a, b, c, d, e, f)), hash)))
    }

    /// Starts a session. Expired sessions are cleaned up on the way.
    pub async fn create_session(
        &self,
        user_id: i32,
        token_hash: &[u8],
        ttl: Duration,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM sessions WHERE expires < now()")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, expires)
             VALUES ($1, $2, now() + make_interval(secs => $3))",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(ttl.as_secs_f64())
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE users SET last_login = now() WHERE id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The user of a valid session. Sessions slide: each use extends them to
    /// `ttl` from now (written at most once a minute).
    pub async fn session_user(&self, token_hash: &[u8], ttl: Duration) -> Result<Option<User>> {
        let sql = format!(
            "WITH s AS (
                 UPDATE sessions SET expires = now() + make_interval(secs => $2)
                 WHERE token_hash = $1 AND expires > now()
                   AND expires < now() + make_interval(secs => $2) - interval '1 minute'
                 RETURNING user_id
             )
             SELECT {USER_COLUMNS} FROM users u
             WHERE NOT u.disabled AND u.id = COALESCE(
                 (SELECT user_id FROM s),
                 (SELECT user_id FROM sessions WHERE token_hash = $1 AND expires > now()))"
        );
        let row: Option<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(token_hash)
            .bind(ttl.as_secs_f64())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(user))
    }

    pub async fn delete_session(&self, token_hash: &[u8]) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_token(
        &self,
        user_id: i32,
        name: &str,
        token_hash: &[u8],
    ) -> Result<ApiToken> {
        let row: (i32, String, f64, Option<f64>) = sqlx::query_as(
            "INSERT INTO api_tokens (user_id, name, token_hash) VALUES ($1, $2, $3)
             RETURNING id, name, extract(epoch FROM created)::float8, extract(epoch FROM last_used)::float8",
        )
        .bind(user_id)
        .bind(name)
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| conflict(e, || format!("a token named {name:?} already exists")))?;
        Ok(token(row))
    }

    /// The user an API token belongs to; records its use (at most once a minute).
    pub async fn token_user(&self, token_hash: &[u8]) -> Result<Option<User>> {
        let sql = format!(
            "WITH t AS (
                 UPDATE api_tokens SET last_used = now()
                 WHERE token_hash = $1 AND (last_used IS NULL OR last_used < now() - interval '1 minute')
             )
             SELECT {USER_COLUMNS} FROM users u JOIN api_tokens a ON a.user_id = u.id
             WHERE a.token_hash = $1 AND NOT u.disabled"
        );
        let row: Option<UserRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(user))
    }

    pub async fn tokens(&self, user_id: i32) -> Result<Vec<ApiToken>> {
        let rows: Vec<(i32, String, f64, Option<f64>)> = sqlx::query_as(
            "SELECT id, name, extract(epoch FROM created)::float8, extract(epoch FROM last_used)::float8
             FROM api_tokens WHERE user_id = $1 ORDER BY lower(name)",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(token).collect())
    }

    /// Revokes a token of the user, by id or name.
    pub async fn delete_token(&self, user_id: i32, token: TokenRef<'_>) -> Result<bool> {
        let query = match token {
            TokenRef::Id(id) => {
                sqlx::query("DELETE FROM api_tokens WHERE user_id = $1 AND id = $2")
                    .bind(user_id)
                    .bind(id)
            }
            TokenRef::Name(name) => {
                sqlx::query("DELETE FROM api_tokens WHERE user_id = $1 AND name = $2")
                    .bind(user_id)
                    .bind(name)
            }
        };
        Ok(query.execute(&self.pool).await?.rows_affected() > 0)
    }
}

/// Identifies a token of a user.
#[derive(Debug, Clone, Copy)]
pub enum TokenRef<'a> {
    Id(i32),
    Name(&'a str),
}

fn token((id, name, created, last_used): (i32, String, f64, Option<f64>)) -> ApiToken {
    ApiToken {
        id,
        name,
        created: from_epoch(created),
        last_used: last_used.map(from_epoch),
    }
}
