use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

/// A skill generated from a meeting, persisted so it can be viewed again from
/// the meeting details page alongside (not instead of) the summary.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SkillArtifact {
    pub id: String,
    pub meeting_id: String,
    pub skill_name: String,
    pub markdown: String,
    pub created_at: String,
}

pub struct SkillArtifactsRepository;

impl SkillArtifactsRepository {
    /// Persist a new skill artifact for a meeting. Returns the new artifact id.
    pub async fn save(
        pool: &SqlitePool,
        meeting_id: &str,
        skill_name: &str,
        markdown: &str,
    ) -> Result<String, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO skill_artifacts (id, meeting_id, skill_name, markdown, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(meeting_id)
        .bind(skill_name)
        .bind(markdown)
        .bind(&created_at)
        .execute(pool)
        .await?;

        Ok(id)
    }

    /// List all skill artifacts for a meeting, newest first.
    pub async fn list_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<SkillArtifact>, sqlx::Error> {
        sqlx::query_as::<_, SkillArtifact>(
            "SELECT id, meeting_id, skill_name, markdown, created_at \
             FROM skill_artifacts WHERE meeting_id = ? ORDER BY created_at DESC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    /// Delete a skill artifact by id. Returns true if a row was removed.
    pub async fn delete(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM skill_artifacts WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
