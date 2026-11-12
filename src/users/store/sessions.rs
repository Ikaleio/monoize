use super::*;

impl UserStore {
    pub async fn create_session(
        &self,
        user_id: &str,
        session_ttl_days: i64,
    ) -> Result<Session, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let token = format!(
            "urp_session_{}",
            uuid::Uuid::new_v4().to_string().replace("-", "")
        );
        let now = Utc::now();
        let expires_at = now + chrono::Duration::days(session_ttl_days);

        self.db
            .write()
            .await
            .execute(self.db.stmt(
                r#"INSERT INTO sessions (id, user_id, token, created_at, expires_at)
                   VALUES ($1, $2, $3, $4, $5)"#,
                vec![
                    id.clone().into(),
                    user_id.into(),
                    token.clone().into(),
                    now.to_rfc3339().into(),
                    expires_at.to_rfc3339().into(),
                ],
            ))
            .await
            .map_err(|e| e.to_string())?;

        Ok(Session {
            id,
            user_id: user_id.to_string(),
            token,
            created_at: now,
            expires_at,
        })
    }

    pub async fn change_password_and_rotate_session(
        &self,
        user_id: &str,
        expected_password_hash: &str,
        new_password: &str,
        session_ttl_days: i64,
    ) -> Result<Session, String> {
        let password_hash = Self::hash_password_async(new_password).await?;
        let now = Utc::now();
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            token: format!(
                "urp_session_{}",
                uuid::Uuid::new_v4().to_string().replace('-', "")
            ),
            created_at: now,
            expires_at: now + chrono::Duration::days(session_ttl_days),
        };

        let write = self.db.write().await;
        let tx = write.begin().await.map_err(|e| e.to_string())?;
        let lock_sql = if self.db.is_postgres() {
            "SELECT password_hash FROM users WHERE id = $1 FOR UPDATE"
        } else {
            "SELECT password_hash FROM users WHERE id = $1"
        };
        let row = tx
            .query_one(self.db.stmt(lock_sql, vec![user_id.into()]))
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "user not found".to_string())?;
        let current_password_hash: String = row
            .try_get("", "password_hash")
            .map_err(|e| e.to_string())?;
        if current_password_hash != expected_password_hash {
            return Err("password changed concurrently".to_string());
        }

        tx.execute(self.db.stmt(
            "UPDATE users SET password_hash = $1, updated_at = $2 WHERE id = $3",
            vec![
                password_hash.into(),
                session.created_at.to_rfc3339().into(),
                user_id.into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
        self.delete_user_sessions_tx(&tx, user_id).await?;
        tx.execute(self.db.stmt(
            r#"INSERT INTO sessions (id, user_id, token, created_at, expires_at)
               VALUES ($1, $2, $3, $4, $5)"#,
            vec![
                session.id.clone().into(),
                session.user_id.clone().into(),
                session.token.clone().into(),
                session.created_at.to_rfc3339().into(),
                session.expires_at.to_rfc3339().into(),
            ],
        ))
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;

        self.api_key_cache.invalidate_by_user_id(user_id);
        Ok(session)
    }

    pub async fn cleanup_expired_sessions(&self) -> Result<u64, String> {
        let result = self
            .db
            .write()
            .await
            .execute(self.db.stmt(
                "DELETE FROM sessions WHERE expires_at <= $1",
                vec![Utc::now().to_rfc3339().into()],
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }

    pub async fn get_session_by_token(&self, token: &str) -> Result<Option<Session>, String> {
        let row = self
            .db
            .read()
            .query_one(self.db.stmt(
                "SELECT id, user_id, token, created_at, expires_at FROM sessions WHERE token = $1",
                vec![token.into()],
            ))
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            let expires_at: String = row.try_get("", "expires_at").map_err(|e| e.to_string())?;
            let expires_at = DateTime::parse_from_rfc3339(&expires_at)
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc);

            if expires_at < Utc::now() {
                self.delete_session(token).await?;
                return Ok(None);
            }

            Ok(Some(Session {
                id: row.try_get("", "id").map_err(|e| e.to_string())?,
                user_id: row.try_get("", "user_id").map_err(|e| e.to_string())?,
                token: row.try_get("", "token").map_err(|e| e.to_string())?,
                created_at: DateTime::parse_from_rfc3339(
                    &row.try_get::<String>("", "created_at")
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
                .with_timezone(&Utc),
                expires_at,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_session(&self, token: &str) -> Result<(), String> {
        self.db
            .write()
            .await
            .execute(
                self.db
                    .stmt("DELETE FROM sessions WHERE token = $1", vec![token.into()]),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(super) async fn delete_user_sessions_tx(
        &self,
        tx: &DatabaseTransaction,
        user_id: &str,
    ) -> Result<(), String> {
        tx.execute(self.db.stmt(
            "DELETE FROM sessions WHERE user_id = $1",
            vec![user_id.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}
