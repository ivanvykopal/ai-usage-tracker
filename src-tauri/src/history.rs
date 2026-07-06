use crate::model::Snapshot;
use rusqlite::{Connection, Result};
use std::path::Path;

pub struct TokenPoint {
    pub ts_ms: i64,
    pub tokens: u64,
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS token_samples (
            ts_ms INTEGER NOT NULL,
            agent TEXT NOT NULL,
            tokens INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_token_samples_agent_ts ON token_samples(agent, ts_ms);
        CREATE TABLE IF NOT EXISTS rate_limit_samples (
            ts_ms INTEGER NOT NULL,
            agent TEXT NOT NULL,
            window TEXT NOT NULL,
            pct REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_rate_limit_samples_agent_window_ts
            ON rate_limit_samples(agent, window, ts_ms);",
    )?;
    Ok(conn)
}

pub fn record_snapshot(conn: &Connection, ts_ms: i64, snapshot: &Snapshot) -> Result<()> {
    for (agent, tokens) in &snapshot.by_agent_tokens {
        conn.execute(
            "INSERT INTO token_samples (ts_ms, agent, tokens) VALUES (?1, ?2, ?3)",
            (ts_ms, agent, *tokens as i64),
        )?;
    }
    for (agent, rl) in &snapshot.usage_limits {
        for (window, pct) in [
            ("five_hour", rl.five_hour_pct),
            ("seven_day", rl.seven_day_pct),
            ("monthly", rl.monthly_pct),
        ] {
            if let Some(pct) = pct {
                conn.execute(
                    "INSERT INTO rate_limit_samples (ts_ms, agent, window, pct) VALUES (?1, ?2, ?3, ?4)",
                    (ts_ms, agent, window, pct),
                )?;
            }
        }
    }
    Ok(())
}

pub fn token_history(conn: &Connection, agent: &str, since_ms: i64) -> Result<Vec<TokenPoint>> {
    let mut stmt = conn.prepare(
        "SELECT ts_ms, tokens FROM token_samples WHERE agent = ?1 AND ts_ms >= ?2 ORDER BY ts_ms ASC",
    )?;
    let rows = stmt.query_map((agent, since_ms), |row| {
        Ok(TokenPoint {
            ts_ms: row.get(0)?,
            tokens: row.get::<_, i64>(1)? as u64,
        })
    })?;
    rows.collect()
}

/// Rate-limit percentage samples for one agent/window, ordered oldest-first —
/// the shape `burn_rate::project_time_to_limit` (Phase 3) consumes directly.
pub fn rate_limit_history(
    conn: &Connection,
    agent: &str,
    window: &str,
    since_ms: i64,
) -> Result<Vec<(i64, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT ts_ms, pct FROM rate_limit_samples WHERE agent = ?1 AND window = ?2 AND ts_ms >= ?3 ORDER BY ts_ms ASC",
    )?;
    let rows = stmt.query_map((agent, window, since_ms), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
    })?;
    rows.collect()
}

pub fn prune_older_than(conn: &Connection, cutoff_ms: i64) -> Result<()> {
    conn.execute("DELETE FROM token_samples WHERE ts_ms < ?1", [cutoff_ms])?;
    conn.execute(
        "DELETE FROM rate_limit_samples WHERE ts_ms < ?1",
        [cutoff_ms],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{build_snapshot, RateLimitInfo};
    use std::collections::BTreeMap;

    fn open_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE token_samples (ts_ms INTEGER, agent TEXT, tokens INTEGER);
             CREATE TABLE rate_limit_samples (ts_ms INTEGER, agent TEXT, window TEXT, pct REAL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn records_and_reads_back_token_totals() {
        let conn = open_memory();
        let snapshot = build_snapshot(vec![], BTreeMap::new());
        // build_snapshot with no sessions has an empty by_agent_tokens map;
        // insert directly to exercise record/query without a full AgentSession.
        conn.execute(
            "INSERT INTO token_samples (ts_ms, agent, tokens) VALUES (1000, 'claude', 500)",
            [],
        )
        .unwrap();
        let points = token_history(&conn, "claude", 0).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].tokens, 500);
        let _ = snapshot; // keep import used
    }

    #[test]
    fn record_snapshot_writes_rate_limit_samples() {
        let conn = open_memory();
        let mut limits = BTreeMap::new();
        limits.insert(
            "codex".to_string(),
            RateLimitInfo {
                source: "codex".into(),
                five_hour_pct: Some(42.0),
                ..Default::default()
            },
        );
        let snapshot = build_snapshot(vec![], limits);
        record_snapshot(&conn, 2000, &snapshot).unwrap();
        let hist = rate_limit_history(&conn, "codex", "five_hour", 0).unwrap();
        assert_eq!(hist, vec![(2000, 42.0)]);
    }

    #[test]
    fn prune_removes_old_rows_only() {
        let conn = open_memory();
        conn.execute(
            "INSERT INTO token_samples (ts_ms, agent, tokens) VALUES (100, 'claude', 1), (5000, 'claude', 2)",
            [],
        )
        .unwrap();
        prune_older_than(&conn, 1000).unwrap();
        let points = token_history(&conn, "claude", 0).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].ts_ms, 5000);
    }
}
