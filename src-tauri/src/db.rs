//! SQLite: история диалогов, факты памяти с векторами, заметки, таймеры,
//! журнал действий и индекс установленных приложений. Файл: %APPDATA%\Jarvis3\jarvis.db.

use chrono::Utc;
use rusqlite::{params, Connection, Row};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::config::app_dir;

pub struct Db(pub Mutex<Connection>);

impl Db {
    pub fn open() -> Result<Self, String> {
        let path: PathBuf = app_dir().join("jarvis.db");
        Self::open_at_inner(&path)
    }

    /// Тестовый конструктор: уникальный файл → нет contention между тестами.
    #[cfg(test)]
    pub fn open_at(path: &std::path::Path) -> Result<Self, String> {
        Self::open_at_inner(path)
    }

    fn open_at_inner(path: &std::path::Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;
            CREATE TABLE IF NOT EXISTS chats(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                created TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
                role TEXT NOT NULL,            -- user | assistant
                content TEXT NOT NULL,
                sources TEXT NOT NULL DEFAULT '[]',
                ts TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS facts(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL UNIQUE,
                vec BLOB NOT NULL,             -- 256×f32, утилита util::embed
                ts TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS notes(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'note',  -- note | voice | document
                audio_path TEXT NOT NULL DEFAULT '',
                ts TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS timers(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                ends_at TEXT NOT NULL,         -- RFC3339
                fired INTEGER NOT NULL DEFAULT 0,
                created TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS action_log(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                tool TEXT NOT NULL,
                args TEXT NOT NULL,
                status TEXT NOT NULL,          -- ok | denied | error | need-confirm
                detail TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS apps(
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                path TEXT NOT NULL,            -- exe / .lnk / uri
                kind TEXT NOT NULL,            -- exe | lnk | uri | uwp | folder
                is_media INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self(Mutex::new(conn)))
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, rusqlite::Error>) -> Result<T, String> {
        let conn = self.0.lock().map_err(|_| "db locked".to_string())?;
        f(&conn).map_err(|e| e.to_string())
    }

    // ---------------------------------------------------------- чаты и сообщения

    pub fn chat_create(&self, title: &str) -> Result<i64, String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO chats(title, created) VALUES (?, ?)",
                params![truncate(title, 60), Utc::now().to_rfc3339()],
            )
            .map(|_| c.last_insert_rowid())
        })
    }

    pub fn chat_last_id(&self) -> Result<Option<i64>, String> {
        self.with(|c| c.query_row("SELECT id FROM chats ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).map(Some).or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        }))
    }

    pub fn message_add(&self, chat_id: i64, role: &str, content: &str, sources: &str) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO messages(chat_id, role, content, sources, ts) VALUES (?, ?, ?, ?, ?)",
                params![chat_id, role, content, sources, Utc::now().to_rfc3339()],
            )
            .map(|_| ())
        })
    }

    /// Последние n сообщений чата в хронологическом порядке.
    pub fn messages_recent(&self, chat_id: i64, n: usize) -> Result<Vec<(String, String)>, String> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT role, content FROM (
                    SELECT id, role, content FROM messages WHERE chat_id = ? ORDER BY id DESC LIMIT ?
                ) ORDER BY id ASC",
            )?;
            let rows = stmt
                .query_map(params![chat_id, n as i64], |r: &Row| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn chats_list(&self) -> Result<Vec<(i64, String, String)>, String> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, title, created FROM chats ORDER BY id DESC LIMIT 50")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // ---------------------------------------------------------- факты (векторная память)

    pub fn fact_add(&self, text: &str) -> Result<bool, String> {
        let text = truncate(text.trim(), 300);
        if text.len() < 3 {
            return Ok(false);
        }
        let vec = crate::util::embed(&text);
        self.with(|c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO facts(text, vec, ts) VALUES (?, ?, ?)",
                params![text, vec_to_blob(&vec), Utc::now().to_rfc3339()],
            )?;
            // Ограничение: храним 300 свежих фактов.
            c.execute(
                "DELETE FROM facts WHERE id NOT IN (SELECT id FROM facts ORDER BY id DESC LIMIT 300)",
                [],
            )
            .map(|_| n > 0)
        })
    }

    pub fn fact_remove_matching(&self, query: &str) -> Result<Vec<String>, String> {
        let rows = self.facts_all()?;
        let q = query.to_lowercase();
        let mut removed = Vec::new();
        let matched: Vec<(i64, String)> = rows
            .into_iter()
            .filter(|(_id, t)| {
                let t_low = t.to_lowercase();
                t_low.contains(&q) || q.contains(&t_low) || crate::util::similarity(&t_low, &q) >= 0.7
            })
            .map(|(id, t)| (id, t))
            .collect();
        for (id, t) in matched {
            self.with(|c| c.execute("DELETE FROM facts WHERE id = ?", params![id]).map(|_| ()))?;
            removed.push(t);
        }
        Ok(removed)
    }

    pub fn facts_all(&self) -> Result<Vec<(i64, String)>, String> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, text FROM facts ORDER BY id DESC")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Семантический поиск фактов: косинусовая близость к запросу.
    pub fn facts_semantic(&self, query: &str, limit: usize) -> Result<Vec<(String, f32)>, String> {
        let qv = crate::util::embed(query);
        let rows = self.facts_all()?;
        let mut scored: Vec<(String, f32)> = rows
            .into_iter()
            .map(|(_id, t)| { let sim = cosine(&qv, &crate::util::embed(&t)); (t, sim) })
            .filter(|(_t, s)| *s > 0.15)
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    // ---------------------------------------------------------- заметки

    pub fn note_add(&self, title: &str, body: &str, kind: &str, audio_path: &str) -> Result<i64, String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO notes(title, body, kind, audio_path, ts) VALUES (?, ?, ?, ?, ?)",
                params![truncate(title, 80), body, kind, audio_path, Utc::now().to_rfc3339()],
            )
            .map(|_| c.last_insert_rowid())
        })
    }

    pub fn notes_recent(&self, n: usize) -> Result<Vec<crate::Note>, String> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT id, title, body, kind, audio_path, ts FROM notes ORDER BY id DESC LIMIT ?")?;
            let rows = stmt
                .query_map(params![n as i64], |r| {
                    Ok(crate::Note {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        body: r.get(2)?,
                        kind: r.get(3)?,
                        audio_path: r.get(4)?,
                        ts: r.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // ---------------------------------------------------------- таймеры

    pub fn timer_add(&self, name: &str, ends_at: &str) -> Result<i64, String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO timers(name, ends_at, created) VALUES (?, ?, ?)",
                params![name, ends_at, Utc::now().to_rfc3339()],
            )
            .map(|_| c.last_insert_rowid())
        })
    }

    pub fn timers_pending(&self) -> Result<Vec<(i64, String, String)>, String> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT id, name, ends_at FROM timers WHERE fired = 0 ORDER BY id")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn timer_cancel(&self, id: i64) -> Result<bool, String> {
        self.with(|c| c.execute("DELETE FROM timers WHERE id = ?", params![id]).map(|n| n > 0))
    }

    pub fn timer_cancel_all(&self) -> Result<usize, String> {
        self.with(|c| c.execute("DELETE FROM timers WHERE fired = 0", []).map(|n| n))
    }

    pub fn timer_mark_fired(&self, id: i64) -> Result<(), String> {
        self.with(|c| c.execute("UPDATE timers SET fired = 1 WHERE id = ?", params![id]).map(|_| ()))
    }

    // ---------------------------------------------------------- журнал действий

    pub fn log_action(&self, tool: &str, args: &str, status: &str, detail: &str) {
        let _ = self.with(|c| {
            c.execute(
                "INSERT INTO action_log(ts, tool, args, status, detail) VALUES (?, ?, ?, ?, ?)",
                params![Utc::now().to_rfc3339(), tool, truncate(args, 400), status, truncate(detail, 400)],
            )
            .map(|_| ())
        });
    }

    pub fn action_log_recent(&self, n: usize) -> Result<Vec<crate::ActionEntry>, String> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT ts, tool, args, status, detail FROM action_log ORDER BY id DESC LIMIT ?")?;
            let rows = stmt
                .query_map(params![n as i64], |r| {
                    Ok(crate::ActionEntry {
                        ts: r.get(0)?,
                        tool: r.get(1)?,
                        args: r.get(2)?,
                        status: r.get(3)?,
                        detail: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // ---------------------------------------------------------- индекс приложений

    pub fn apps_replace(&self, apps: &[(String, String, String, bool)]) -> Result<usize, String> {
        self.with(|c| {
            c.execute("DELETE FROM apps", [])?;
            let mut stmt = c.prepare("INSERT OR IGNORE INTO apps(name, path, kind, is_media) VALUES (?, ?, ?, ?)")?;
            for (name, path, kind, is_media) in apps {
                stmt.execute(params![name.to_lowercase(), path, kind, *is_media as i64])?;
            }
            Ok(apps.len())
        })
    }

    pub fn apps_search(&self, query: &str) -> Result<Vec<crate::AppEntry>, String> {
        let rows = self.with(|c| {
            let mut stmt = c.prepare("SELECT name, path, kind, is_media FROM apps")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(crate::AppEntry {
                        name: r.get(0)?,
                        path: r.get(1)?,
                        kind: r.get(2)?,
                        is_media: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        Ok(crate::util::best_matches(query, rows))
    }

    pub fn apps_all(&self) -> Result<Vec<crate::AppEntry>, String> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT name, path, kind, is_media FROM apps ORDER BY name")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(crate::AppEntry {
                        name: r.get(0)?,
                        path: r.get(1)?,
                        kind: r.get(2)?,
                        is_media: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn apps_media(&self) -> Result<Vec<crate::AppEntry>, String> {
        Ok(self.apps_all()?.into_iter().filter(|a| a.is_media).collect())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn vec_to_blob(v: &[f32; 256]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn cosine(a: &[f32; 256], b: &[f32; 256]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    dot.clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    // Тесты работают во временном APPDATA, чтобы не трогать реальные данные.
    fn test_env() -> &'static () {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| {
            let tmp = std::env::temp_dir().join(format!("jarvis3-test-{}", std::process::id()));
            std::fs::create_dir_all(&tmp).unwrap();
            std::env::set_var("APPDATA", &tmp);
        })
    }

    fn tmp_db() -> Db {
        test_env();
        let path = std::env::temp_dir().join(format!("jarvis3-test-{}-{}.db", std::process::id(), crate::util::short_id()));
        Db::open_at(&path).unwrap()
    }

    #[test]
    fn chats_roundtrip() {
        let db = tmp_db();
        let id = db.chat_create("тест").unwrap();
        db.message_add(id, "user", "привет", "[]").unwrap();
        db.message_add(id, "assistant", "здравствуй", "[]").unwrap();
        let msgs = db.messages_recent(id, 10).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].0, "user");
    }

    #[test]
    fn facts_semantic_recall() {
        let db = tmp_db();
        assert!(db.fact_add("Пользователь предпочитает Spotify для музыки").unwrap());
        assert!(db.fact_add("У пользователя день рождения 12 мая").unwrap());
        let hits = db.facts_semantic("какую музыку слушать пользователь предпочитает spotify", 3).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].0.contains("Spotify"));
    }

    #[test]
    fn facts_dedupe_and_remove() {
        let db = tmp_db();
        assert!(db.fact_add("Люблю кофе").unwrap());
        assert!(!db.fact_add("Люблю кофе").unwrap());
        let removed = db.fact_remove_matching("кофе").unwrap();
        assert_eq!(removed.len(), 1);
        assert!(db.facts_all().unwrap().is_empty());
    }

    #[test]
    fn action_log_and_apps() {
        let db = tmp_db();
        db.log_action("open_app", "{\"name\":\"calc\"}", "ok", "");
        let log = db.action_log_recent(5).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].tool, "open_app");

        db.apps_replace(&[
            ("калькулятор".into(), "calc.exe".into(), "exe".into(), false),
            ("spotify".into(), "spotify:".into(), "uri".into(), true),
        ])
        .unwrap();
        let found = db.apps_search("спотифай").unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].is_media);
    }
}
