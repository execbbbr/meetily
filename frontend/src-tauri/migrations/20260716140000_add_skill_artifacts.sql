-- Skill artifacts: skills generated from a meeting, stored alongside (not
-- replacing) the meeting summary. Unlike summary_processes (one row per
-- meeting), a meeting can have multiple skill artifacts.
CREATE TABLE IF NOT EXISTS skill_artifacts (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    markdown TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_skill_artifacts_meeting_id
    ON skill_artifacts(meeting_id);
