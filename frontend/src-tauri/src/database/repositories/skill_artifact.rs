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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Build an in-memory DB with just the tables these tests need.
    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Minimal schema: meetings (for the FK) + skill_artifacts.
        sqlx::query("CREATE TABLE meetings (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE skill_artifacts (\
                id TEXT PRIMARY KEY, \
                meeting_id TEXT NOT NULL, \
                skill_name TEXT NOT NULL, \
                markdown TEXT NOT NULL, \
                created_at TEXT NOT NULL, \
                FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_meeting(pool: &SqlitePool, id: &str) {
        sqlx::query("INSERT INTO meetings (id) VALUES (?)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn save_then_list_returns_the_artifact() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;

        let id = SkillArtifactsRepository::save(&pool, "m1", "My Skill", "# body")
            .await
            .unwrap();
        assert!(!id.is_empty());

        let list = SkillArtifactsRepository::list_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].skill_name, "My Skill");
        assert_eq!(list[0].markdown, "# body");
        assert_eq!(list[0].meeting_id, "m1");
    }

    #[tokio::test]
    async fn list_is_scoped_to_meeting() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        insert_meeting(&pool, "m2").await;

        SkillArtifactsRepository::save(&pool, "m1", "A", "a").await.unwrap();
        SkillArtifactsRepository::save(&pool, "m2", "B", "b").await.unwrap();

        let m1 = SkillArtifactsRepository::list_for_meeting(&pool, "m1").await.unwrap();
        assert_eq!(m1.len(), 1);
        assert_eq!(m1[0].skill_name, "A");
    }

    #[tokio::test]
    async fn multiple_artifacts_per_meeting_newest_first() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;

        // created_at is an RFC3339 string; insert with distinct timestamps to
        // make ordering deterministic (bypass save() so we control created_at).
        for (id, name, ts) in [
            ("id-old", "Old", "2026-01-01T00:00:00+00:00"),
            ("id-new", "New", "2026-06-01T00:00:00+00:00"),
        ] {
            sqlx::query(
                "INSERT INTO skill_artifacts (id, meeting_id, skill_name, markdown, created_at) \
                 VALUES (?, 'm1', ?, 'x', ?)",
            )
            .bind(id)
            .bind(name)
            .bind(ts)
            .execute(&pool)
            .await
            .unwrap();
        }

        let list = SkillArtifactsRepository::list_for_meeting(&pool, "m1").await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].skill_name, "New"); // newest first
        assert_eq!(list[1].skill_name, "Old");
    }

    #[tokio::test]
    async fn delete_removes_only_the_target() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;

        let id1 = SkillArtifactsRepository::save(&pool, "m1", "A", "a").await.unwrap();
        let _id2 = SkillArtifactsRepository::save(&pool, "m1", "B", "b").await.unwrap();

        assert!(SkillArtifactsRepository::delete(&pool, &id1).await.unwrap());
        let list = SkillArtifactsRepository::list_for_meeting(&pool, "m1").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].skill_name, "B");
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_false() {
        let pool = setup_pool().await;
        assert!(!SkillArtifactsRepository::delete(&pool, "does-not-exist").await.unwrap());
    }

    #[tokio::test]
    async fn list_empty_when_no_artifacts() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        let list = SkillArtifactsRepository::list_for_meeting(&pool, "m1").await.unwrap();
        assert!(list.is_empty());
    }
}
