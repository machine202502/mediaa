use crate::clock::Clock;
use crate::state::{self, Child, Filter, KindStat, NewTask, Op, Outcome, Stats, Task, World};
use crate::{Error, Result, TaskId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

const SCHEMA: &str = "
DROP TABLE IF EXISTS task_deps;
DROP TABLE IF EXISTS tasks;
DROP TABLE IF EXISTS kind_stats;
DROP TABLE IF EXISTS meta;
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    input TEXT NOT NULL,
    priority INTEGER NOT NULL,
    status TEXT NOT NULL,
    cursor TEXT NOT NULL,
    partial TEXT NOT NULL,
    result TEXT,
    progress REAL NOT NULL,
    error TEXT,
    attempts INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    parent INTEGER,
    scope TEXT,
    inline INTEGER NOT NULL,
    started_ms INTEGER,
    finished_ms INTEGER,
    stats TEXT NOT NULL,
    detail TEXT NOT NULL,
    units REAL NOT NULL,
    resource TEXT,
    memory INTEGER NOT NULL,
    model TEXT,
    role TEXT NOT NULL,
    gen INTEGER NOT NULL,
    yield_cont INTEGER NOT NULL,
    heartbeat_ms INTEGER,
    stop_intent TEXT,
    note TEXT
);
CREATE INDEX tasks_parent ON tasks(parent);
CREATE INDEX tasks_status ON tasks(status);
CREATE TABLE task_deps (
    task INTEGER NOT NULL,
    need INTEGER NOT NULL,
    PRIMARY KEY (task, need)
);
CREATE TABLE kind_stats (
    kind TEXT PRIMARY KEY,
    runs INTEGER NOT NULL,
    ms INTEGER NOT NULL,
    units REAL NOT NULL
);
CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
PRAGMA user_version = 2;
";

const TASK_INSERT: &str = "INSERT INTO tasks (
        id, kind, title, input, priority, status, cursor, partial, result, progress, error, attempts,
        created_ms, updated_ms, parent, scope, inline, started_ms, finished_ms, stats, detail, units,
        resource, memory, model, role, gen, yield_cont, heartbeat_ms, stop_intent, note
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
        ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
        ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31
    )";

const TASK_UPDATE: &str = "UPDATE tasks SET
        kind = ?2, title = ?3, input = ?4, priority = ?5, status = ?6, cursor = ?7, partial = ?8,
        result = ?9, progress = ?10, error = ?11, attempts = ?12, created_ms = ?13, updated_ms = ?14,
        parent = ?15, scope = ?16, inline = ?17, started_ms = ?18, finished_ms = ?19, stats = ?20,
        detail = ?21, units = ?22, resource = ?23, memory = ?24, model = ?25, role = ?26, gen = ?27,
        yield_cont = ?28, heartbeat_ms = ?29, stop_intent = ?30, note = ?31
    WHERE id = ?1";

macro_rules! task_params {
    ($task:expr) => {{
        let task = $task;
        rusqlite::params![
            task.id,
            task.kind,
            task.title,
            task.input.to_string(),
            task.priority,
            task.status.as_str(),
            task.cursor.to_string(),
            task.partial.to_string(),
            task.result.as_ref().map(Value::to_string),
            f64::from(task.progress),
            task.error,
            task.attempts,
            task.created_ms,
            task.updated_ms,
            task.parent,
            task.scope,
            if task.inline { 1 } else { 0 },
            task.started_ms,
            task.finished_ms,
            serde_json::to_string(&task.stats)?,
            task.detail.to_string(),
            task.units,
            task.resource.map(|resource| resource.as_str()),
            task.memory as i64,
            task.model,
            task.role.code(),
            task.gen,
            if task.yield_cont { 1 } else { 0 },
            task.heartbeat_ms,
            task.stop_intent.map(|stop| stop.as_str()),
            match &task.note {
                Some(note) => Some(serde_json::to_string(note)?),
                None => None,
            },
        ]
    }};
}

pub trait Store: Send + Sync {
    fn apply(&self, op: Op) -> Result<Outcome>;
    fn world(&self) -> Result<World>;
    fn now_ms(&self) -> i64;

    fn enqueue(&self, task: NewTask) -> Result<TaskId> {
        let out = self.apply(Op::Enqueue(task))?;
        out.ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("enqueue returned no id".into()))
    }

    fn enqueue_tree(&self, root: NewTask, children: Vec<Child>) -> Result<(TaskId, Vec<TaskId>)> {
        let out = self.apply(Op::EnqueueTree { root, children })?;
        let root_id = out
            .ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("enqueue_tree returned no id".into()))?;
        let direct = out.ids.iter().skip(1).take(out.changed).copied().collect();
        Ok((root_id, direct))
    }

    fn enqueue_once(&self, task: NewTask) -> Result<TaskId> {
        let out = self.apply(Op::EnqueueOnce(task))?;
        out.ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("enqueue_once returned no id".into()))
    }

    fn get(&self, id: TaskId) -> Result<Option<Task>> {
        Ok(self.world()?.task(id).cloned())
    }

    fn list(&self) -> Result<Vec<Task>> {
        let mut tasks = self.world()?.tasks;
        state::sort_tasks(&mut tasks);
        Ok(tasks)
    }

    fn top_level(&self, filter: &Filter) -> Result<Vec<Task>> {
        Ok(state::top_level(&self.world()?.tasks, filter))
    }

    fn children(&self, id: TaskId) -> Result<Vec<Task>> {
        let world = self.world()?;
        let mut kids: Vec<Task> = world
            .tasks
            .into_iter()
            .filter(|task| task.parent == Some(id))
            .collect();
        kids.sort_by_key(|task| task.id);
        Ok(kids)
    }

    fn subtree(&self, id: TaskId) -> Result<Vec<Task>> {
        let world = self.world()?;
        let ids = state::subtree_ids(&world, id);
        Ok(ids
            .iter()
            .filter_map(|id| world.task(*id).cloned())
            .collect())
    }

    fn needs(&self, id: TaskId) -> Result<Vec<TaskId>> {
        let mut needs: Vec<TaskId> = self
            .world()?
            .deps
            .into_iter()
            .filter(|(task, _)| *task == id)
            .map(|(_, need)| need)
            .collect();
        needs.sort_unstable();
        Ok(needs)
    }

    fn kind_stats(&self) -> Result<Vec<KindStat>> {
        let mut stats = self.world()?.kind_stats;
        stats.sort_by(|a, b| a.kind.cmp(&b.kind));
        Ok(stats)
    }

    fn claim(&self, id: TaskId) -> Result<Outcome> {
        self.apply(Op::Claim(id))
    }

    fn checkpoint(
        &self,
        id: TaskId,
        cursor: &Value,
        partial: &Value,
        progress: f32,
    ) -> Result<Outcome> {
        let gen = self.gen_of(id)?;
        self.apply(Op::Checkpoint {
            id,
            gen,
            cursor: cursor.clone(),
            partial: partial.clone(),
            progress,
        })
    }

    fn complete(&self, id: TaskId, result: &Value) -> Result<Outcome> {
        let gen = self.gen_of(id)?;
        self.apply(Op::Complete {
            id,
            gen,
            result: result.clone(),
            children: Vec::new(),
        })
    }

    fn complete_with(&self, id: TaskId, result: &Value, children: Vec<Child>) -> Result<Outcome> {
        let gen = self.gen_of(id)?;
        self.apply(Op::Complete {
            id,
            gen,
            result: result.clone(),
            children,
        })
    }

    fn fail(&self, id: TaskId, error: &str) -> Result<Outcome> {
        let gen = self.gen_of(id)?;
        self.apply(Op::Fail {
            id,
            gen,
            error: error.to_string(),
        })
    }

    fn pause(&self, id: TaskId) -> Result<Outcome> {
        self.apply(Op::Pause(id))
    }

    fn resume(&self, id: TaskId) -> Result<Outcome> {
        self.apply(Op::Resume(id))
    }

    fn cancel(&self, id: TaskId) -> Result<Outcome> {
        self.apply(Op::Cancel(id))
    }

    fn cancel_scope(&self, scope: &str) -> Result<Outcome> {
        self.apply(Op::CancelScope(scope.to_string()))
    }

    fn cancel_analysis(&self, scope: &str) -> Result<Outcome> {
        self.apply(Op::CancelAnalysis(scope.to_string()))
    }

    fn restart(&self, id: TaskId) -> Result<Outcome> {
        self.apply(Op::Restart(id))
    }

    fn begin_inline(
        &self,
        kind: &str,
        title: &str,
        input: &Value,
        parent: Option<TaskId>,
        scope: Option<&str>,
    ) -> Result<TaskId> {
        let out = self.apply(Op::BeginInline {
            kind: kind.to_string(),
            title: title.to_string(),
            input: input.clone(),
            parent,
            scope: scope.map(str::to_string),
        })?;
        out.ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("inline returned no id".into()))
    }

    fn add_stats(&self, id: TaskId, delta: &Stats) -> Result<Outcome> {
        self.apply(Op::AddStats {
            id,
            delta: delta.clone(),
        })
    }

    fn set_detail(&self, id: TaskId, detail: &Value) -> Result<Outcome> {
        self.apply(Op::SetDetail {
            id,
            detail: detail.clone(),
        })
    }

    fn set_priority(&self, id: TaskId, priority: i64) -> Result<Outcome> {
        self.apply(Op::SetPriority { id, priority })
    }

    fn remove(&self, id: TaskId) -> Result<Outcome> {
        self.apply(Op::Remove(id))
    }

    fn clear_done(&self) -> Result<Outcome> {
        self.apply(Op::ClearDone)
    }

    fn purge_older_than(&self, cutoff_ms: i64) -> Result<Outcome> {
        self.apply(Op::Purge { cutoff_ms })
    }

    fn recover(&self) -> Result<Outcome> {
        self.apply(Op::Recover)
    }

    fn gen_of(&self, id: TaskId) -> Result<i64> {
        Ok(self.world()?.task(id).map(|task| task.gen).unwrap_or(0))
    }
}

pub struct MemStore {
    world: Mutex<World>,
    clock: Arc<dyn Clock>,
}

impl MemStore {
    pub fn new(clock: Arc<dyn Clock>) -> MemStore {
        MemStore {
            world: Mutex::new(World::default()),
            clock,
        }
    }
}

impl Store for MemStore {
    fn apply(&self, op: Op) -> Result<Outcome> {
        let mut guard = self.world.lock().expect("queue lock");
        let (next, out) = state::apply(guard.clone(), op, self.clock.now_ms())?;
        *guard = next;
        Ok(out)
    }

    fn world(&self) -> Result<World> {
        Ok(self.world.lock().expect("queue lock").clone())
    }

    fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Writes {
    pub inserted: u64,
    pub updated: u64,
    pub removed: u64,
    pub statements: u64,
}

impl Writes {
    fn merge(&mut self, other: Writes) {
        self.inserted += other.inserted;
        self.updated += other.updated;
        self.removed += other.removed;
        self.statements += other.statements;
    }
}

struct Db {
    conn: Connection,
    world: World,
    writes: Writes,
}

struct OldRows {
    ids: HashSet<TaskId>,
    deps: Vec<(TaskId, TaskId)>,
    kind_stats: Vec<KindStat>,
    next_id: i64,
}

pub struct SqliteStore {
    db: Mutex<Db>,
    clock: Arc<dyn Clock>,
}

impl SqliteStore {
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<SqliteStore> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != 2 {
            conn.execute_batch(SCHEMA)?;
        }
        let world = load(&conn)?;
        Ok(SqliteStore {
            db: Mutex::new(Db {
                conn,
                world,
                writes: Writes::default(),
            }),
            clock,
        })
    }

    pub fn writes(&self) -> Writes {
        self.db.lock().expect("queue lock").writes
    }
}

impl Store for SqliteStore {
    fn apply(&self, op: Op) -> Result<Outcome> {
        let mut guard = self.db.lock().expect("queue lock");
        let db = &mut *guard;
        let tx = db.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = self.clock.now_ms();
        let old = OldRows {
            ids: db.world.tasks.iter().map(|task| task.id).collect(),
            deps: db.world.deps.clone(),
            kind_stats: db.world.kind_stats.clone(),
            next_id: db.world.next_id,
        };
        let world = std::mem::take(&mut db.world);
        let (world, out) = match state::apply(world, op, now) {
            Ok(applied) => applied,
            Err(err) => {
                drop(tx);
                db.world = load(&db.conn)?;
                return Err(err);
            }
        };
        let writes = match persist(&tx, &old, &world, now) {
            Ok(writes) => writes,
            Err(err) => {
                drop(tx);
                db.world = load(&db.conn)?;
                return Err(err);
            }
        };
        if let Err(err) = tx.commit() {
            db.world = load(&db.conn)?;
            return Err(err.into());
        }
        db.writes.merge(writes);
        db.world = world;
        Ok(out)
    }

    fn world(&self) -> Result<World> {
        Ok(self.db.lock().expect("queue lock").world.clone())
    }

    fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }
}

fn load(conn: &Connection) -> Result<World> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, title, input, priority, status, cursor, partial, result, progress, error, attempts,
            created_ms, updated_ms, parent, scope, inline, started_ms, finished_ms, stats, detail, units,
            resource, memory, model, role, gen, yield_cont, heartbeat_ms, stop_intent, note
         FROM tasks ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next()? {
        tasks.push(read_task(row)?);
    }
    drop(rows);
    drop(stmt);
    let mut deps = Vec::new();
    let mut stmt = conn.prepare("SELECT task, need FROM task_deps ORDER BY task, need")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        deps.push((row.get(0)?, row.get(1)?));
    }
    drop(rows);
    drop(stmt);
    let mut kind_stats = Vec::new();
    let mut stmt = conn.prepare("SELECT kind, runs, ms, units FROM kind_stats")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        kind_stats.push(KindStat {
            kind: row.get(0)?,
            runs: row.get(1)?,
            ms: row.get(2)?,
            units: row.get(3)?,
        });
    }
    drop(rows);
    drop(stmt);
    let stored: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = 'next_id'", [], |row| {
            row.get(0)
        })
        .optional()?;
    let max_id = tasks.iter().map(|task| task.id).max().unwrap_or(0);
    let next_id = stored.unwrap_or(1).max(max_id + 1);
    Ok(World {
        tasks,
        deps,
        kind_stats,
        next_id,
    })
}

fn load_task(conn: &Connection, id: TaskId) -> Result<Option<Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, title, input, priority, status, cursor, partial, result, progress, error, attempts,
            created_ms, updated_ms, parent, scope, inline, started_ms, finished_ms, stats, detail, units,
            resource, memory, model, role, gen, yield_cont, heartbeat_ms, stop_intent, note
         FROM tasks WHERE id = ?1",
    )?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(row) => Ok(Some(read_task(row)?)),
        None => Ok(None),
    }
}

fn read_task(row: &rusqlite::Row<'_>) -> Result<Task> {
    let status: String = row.get(5)?;
    let resource: Option<String> = row.get(22)?;
    let stop: Option<String> = row.get(29)?;
    let memory: i64 = row.get(23)?;
    Ok(Task {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        input: parse_json(&row.get::<_, String>(3)?)?,
        priority: row.get(4)?,
        status: state::Status::parse(&status)?,
        cursor: parse_json(&row.get::<_, String>(6)?)?,
        partial: parse_json(&row.get::<_, String>(7)?)?,
        result: match row.get::<_, Option<String>>(8)? {
            Some(text) => Some(parse_json(&text)?),
            None => None,
        },
        progress: row.get::<_, f64>(9)? as f32,
        error: row.get(10)?,
        attempts: row.get(11)?,
        created_ms: row.get(12)?,
        updated_ms: row.get(13)?,
        parent: row.get(14)?,
        scope: row.get(15)?,
        inline: row.get::<_, i64>(16)? != 0,
        started_ms: row.get(17)?,
        finished_ms: row.get(18)?,
        stats: serde_json::from_str(&row.get::<_, String>(19)?)?,
        detail: parse_json(&row.get::<_, String>(20)?)?,
        units: row.get(21)?,
        resource: match resource {
            Some(text) => Some(state::Resource::parse(&text)?),
            None => None,
        },
        memory: memory.max(0) as u64,
        model: row.get(24)?,
        role: state::Role::parse(&row.get::<_, String>(25)?),
        gen: row.get(26)?,
        yield_cont: row.get::<_, i64>(27)? != 0,
        heartbeat_ms: row.get(28)?,
        stop_intent: match stop {
            Some(text) => Some(state::StopIntent::parse(&text)?),
            None => None,
        },
        note: match row.get::<_, Option<String>>(30)? {
            Some(text) => Some(serde_json::from_str(&text)?),
            None => None,
        },
    })
}

fn parse_json(text: &str) -> Result<Value> {
    Ok(serde_json::from_str(text)?)
}

fn persist(conn: &Connection, old: &OldRows, new: &World, now: i64) -> Result<Writes> {
    let mut writes = Writes::default();
    for task in &new.tasks {
        if !old.ids.contains(&task.id) {
            conn.execute(TASK_INSERT, task_params!(task))?;
            writes.inserted += 1;
            writes.statements += 1;
            continue;
        }
        if task.updated_ms != now && task.heartbeat_ms != Some(now) {
            continue;
        }
        match load_task(conn, task.id)? {
            Some(previous) if previous != *task => {
                conn.execute(TASK_UPDATE, task_params!(task))?;
                writes.updated += 1;
                writes.statements += 1;
            }
            Some(_) => {}
            None => {
                conn.execute(TASK_INSERT, task_params!(task))?;
                writes.inserted += 1;
                writes.statements += 1;
            }
        }
    }
    let new_ids: HashSet<TaskId> = new.tasks.iter().map(|task| task.id).collect();
    for id in &old.ids {
        if !new_ids.contains(id) {
            conn.execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id])?;
            writes.removed += 1;
            writes.statements += 1;
        }
    }

    let old_deps: HashSet<(TaskId, TaskId)> = old.deps.iter().copied().collect();
    let new_deps: HashSet<(TaskId, TaskId)> = new.deps.iter().copied().collect();
    for (task, need) in &new.deps {
        if !old_deps.contains(&(*task, *need)) {
            conn.execute(
                "INSERT INTO task_deps (task, need) VALUES (?1, ?2)",
                rusqlite::params![task, need],
            )?;
            writes.inserted += 1;
            writes.statements += 1;
        }
    }
    for (task, need) in &old.deps {
        if !new_deps.contains(&(*task, *need)) {
            conn.execute(
                "DELETE FROM task_deps WHERE task = ?1 AND need = ?2",
                rusqlite::params![task, need],
            )?;
            writes.removed += 1;
            writes.statements += 1;
        }
    }

    let old_stats: HashMap<&str, &KindStat> = old
        .kind_stats
        .iter()
        .map(|stat| (stat.kind.as_str(), stat))
        .collect();
    for stat in &new.kind_stats {
        match old_stats.get(stat.kind.as_str()) {
            None => {
                conn.execute(
                    "INSERT INTO kind_stats (kind, runs, ms, units) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![stat.kind, stat.runs, stat.ms, stat.units],
                )?;
                writes.inserted += 1;
                writes.statements += 1;
            }
            Some(prev) if *prev != stat => {
                conn.execute(
                    "UPDATE kind_stats SET runs = ?2, ms = ?3, units = ?4 WHERE kind = ?1",
                    rusqlite::params![stat.kind, stat.runs, stat.ms, stat.units],
                )?;
                writes.updated += 1;
                writes.statements += 1;
            }
            Some(_) => {}
        }
    }
    let new_stats: HashSet<&str> = new
        .kind_stats
        .iter()
        .map(|stat| stat.kind.as_str())
        .collect();
    for stat in &old.kind_stats {
        if !new_stats.contains(stat.kind.as_str()) {
            conn.execute(
                "DELETE FROM kind_stats WHERE kind = ?1",
                rusqlite::params![stat.kind],
            )?;
            writes.removed += 1;
            writes.statements += 1;
        }
    }

    if old.next_id != new.next_id {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('next_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![new.next_id],
        )?;
        writes.updated += 1;
        writes.statements += 1;
    }
    Ok(writes)
}

#[cfg(test)]
fn save(conn: &Connection, world: &World) -> Result<()> {
    for task in &world.tasks {
        conn.execute(TASK_INSERT, task_params!(task))?;
    }
    for (task, need) in &world.deps {
        conn.execute(
            "INSERT INTO task_deps (task, need) VALUES (?1, ?2)",
            rusqlite::params![task, need],
        )?;
    }
    for stat in &world.kind_stats {
        conn.execute(
            "INSERT INTO kind_stats (kind, runs, ms, units) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![stat.kind, stat.runs, stat.ms, stat.units],
        )?;
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('next_id', ?1)",
        rusqlite::params![world.next_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::state::{Resource, Role, Status};
    use serde_json::json;
    use std::time::Instant;

    fn small_job(seed: u64) -> NewTask {
        NewTask::new("transcribe", "t", &json!({ "n": seed }))
            .resource(Resource::Llm)
            .units(2.0)
            .scope("s0")
    }

    fn next(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *seed >> 33
    }

    fn assert_same_rows(a: &World, b: &World) {
        assert_eq!(a.next_id, b.next_id, "next_id");
        let mut ta = a.tasks.clone();
        let mut tb = b.tasks.clone();
        ta.sort_by_key(|task| task.id);
        tb.sort_by_key(|task| task.id);
        assert_eq!(ta, tb, "task rows");
        let mut da = a.deps.clone();
        let mut db = b.deps.clone();
        da.sort_unstable();
        db.sort_unstable();
        assert_eq!(da, db, "dependency rows");
        let mut sa = a.kind_stats.clone();
        let mut sb = b.kind_stats.clone();
        sa.sort_by(|x, y| x.kind.cmp(&y.kind));
        sb.sort_by(|x, y| x.kind.cmp(&y.kind));
        assert_eq!(sa, sb, "kind stat rows");
    }

    fn row_counts(conn: &Connection) -> (i64, i64, i64, i64) {
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        (
            count("tasks"),
            count("task_deps"),
            count("kind_stats"),
            count("meta"),
        )
    }

    #[test]
    fn random_ops_leave_the_file_equal_to_a_fresh_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.db");
        let clock = Arc::new(FakeClock::new(1_000));
        let store = SqliteStore::open(&path, clock.clone()).unwrap();
        let mut seed = 0x5eed_1234_abcd_9876u64;
        for step in 0..400u64 {
            if step % 5 == 0 {
                clock.advance(37);
            }
            let world = store.world().unwrap();
            let ids: Vec<TaskId> = world.tasks.iter().map(|task| task.id).collect();
            let pick = |seed: &mut u64| -> Option<TaskId> {
                if ids.is_empty() {
                    None
                } else {
                    Some(ids[(next(seed) as usize) % ids.len()])
                }
            };
            let gen_of = |id: Option<TaskId>| {
                id.and_then(|id| world.task(id))
                    .map(|task| task.gen)
                    .unwrap_or(0)
            };
            let roll = next(&mut seed) % 24;
            let op = match roll {
                0 | 1 => Op::Enqueue(small_job(next(&mut seed))),
                2 => Op::EnqueueOnce(small_job(next(&mut seed))),
                3 => Op::EnqueueTree {
                    root: NewTask::new("analysis", "root", &json!({}))
                        .composite()
                        .scope("s0"),
                    children: vec![
                        Child::new(small_job(next(&mut seed))),
                        Child::new(small_job(next(&mut seed))).after(&[0]),
                    ],
                },
                4 | 5 => match pick(&mut seed) {
                    Some(id) => Op::Claim(id),
                    None => Op::Enqueue(small_job(next(&mut seed))),
                },
                6 | 7 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::Progress {
                        id,
                        gen: gen_of(Some(id)),
                        progress: (next(&mut seed) % 101) as f32 / 100.0,
                    }
                }
                8 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::Checkpoint {
                        id,
                        gen: gen_of(Some(id)),
                        cursor: json!({ "chunk": next(&mut seed) % 5 }),
                        partial: json!({ "text": "x".repeat((next(&mut seed) % 200) as usize) }),
                        progress: (next(&mut seed) % 101) as f32 / 100.0,
                    }
                }
                9 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::Complete {
                        id,
                        gen: gen_of(Some(id)),
                        result: json!({ "ok": step }),
                        children: if next(&mut seed).is_multiple_of(2) {
                            Vec::new()
                        } else {
                            vec![Child::new(small_job(next(&mut seed)))]
                        },
                    }
                }
                10 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::Fail {
                        id,
                        gen: gen_of(Some(id)),
                        error: "model crashed".into(),
                    }
                }
                11 => match pick(&mut seed) {
                    Some(id) => Op::Pause(id),
                    None => Op::Enqueue(small_job(next(&mut seed))),
                },
                12 => match pick(&mut seed) {
                    Some(id) => Op::Resume(id),
                    None => Op::Enqueue(small_job(next(&mut seed))),
                },
                13 => match pick(&mut seed) {
                    Some(id) => Op::Cancel(id),
                    None => Op::Enqueue(small_job(next(&mut seed))),
                },
                14 => match pick(&mut seed) {
                    Some(id) => Op::Restart(id),
                    None => Op::Enqueue(small_job(next(&mut seed))),
                },
                15 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::AddStats {
                        id,
                        delta: Stats {
                            model_calls: 1,
                            prompt_tokens: next(&mut seed),
                            completion_tokens: 3,
                            model_ms: 40,
                            models: vec!["qwen".into()],
                        },
                    }
                }
                16 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::SetDetail {
                        id,
                        detail: json!({ "tool": next(&mut seed) % 4 }),
                    }
                }
                17 => {
                    let id = pick(&mut seed).unwrap_or(0);
                    Op::SetPriority {
                        id,
                        priority: (next(&mut seed) % 10) as i64,
                    }
                }
                18 => Op::BeginInline {
                    kind: "tool".into(),
                    title: "search".into(),
                    input: json!({ "q": next(&mut seed) % 9 }),
                    parent: pick(&mut seed),
                    scope: Some("s0".into()),
                },
                19 => match pick(&mut seed) {
                    Some(id) => Op::Remove(id),
                    None => Op::Enqueue(small_job(next(&mut seed))),
                },
                20 => Op::Recover,
                21 => Op::Reap {
                    older_than_ms: clock.now_ms() - (next(&mut seed) % 3_000) as i64,
                },
                22 => Op::CancelScope("s0".into()),
                _ => match next(&mut seed) % 4 {
                    0 => Op::ClearDone,
                    1 => Op::Purge {
                        cutoff_ms: clock.now_ms(),
                    },
                    2 => Op::Heartbeat {
                        id: pick(&mut seed).unwrap_or(0),
                        gen: 0,
                    },
                    _ => Op::Yield {
                        id: pick(&mut seed).unwrap_or(0),
                        gen: gen_of(pick(&mut seed)),
                    },
                },
            };
            // A rejected or invalid op is part of the fuzz: the cache must survive it too.
            let _ = store.apply(op);
        }

        let world = store.db.lock().unwrap().world.clone();
        let stored = {
            let db = store.db.lock().unwrap();
            load(&db.conn).unwrap()
        };
        assert_same_rows(&stored, &world);
        assert!(!world.tasks.is_empty(), "the fuzz leaves work behind");
        let writes = store.writes();
        assert!(writes.inserted > 0, "the fuzz inserted rows");
        assert!(writes.updated > 0, "the fuzz changed rows");
        assert!(writes.removed > 0, "the fuzz removed rows");

        let fresh_path = dir.path().join("fresh.db");
        let conn = Connection::open(&fresh_path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        save(&conn, &world).unwrap();
        let fresh = load(&conn).unwrap();
        assert_same_rows(&stored, &fresh);
        assert_eq!(row_counts(&conn), row_counts(&store.db.lock().unwrap().conn));
    }

    fn done_task(id: TaskId, partial: &Value, at: i64) -> Task {
        Task {
            id,
            kind: "transcribe".into(),
            title: format!("task {id}"),
            input: json!({ "path": format!("/rec/{id}") }),
            priority: 0,
            status: Status::Done,
            cursor: json!({ "chunk": 9 }),
            partial: partial.clone(),
            result: None,
            progress: 1.0,
            error: None,
            note: None,
            attempts: 1,
            created_ms: at,
            updated_ms: at,
            parent: None,
            scope: None,
            inline: false,
            started_ms: Some(at),
            finished_ms: Some(at),
            stats: Stats::default(),
            detail: Value::Null,
            units: 1.0,
            resource: Some(Resource::Speech),
            memory: 0,
            model: None,
            role: Role::Work,
            gen: 1,
            yield_cont: false,
            heartbeat_ms: None,
            stop_intent: None,
        }
    }

    #[test]
    fn a_progress_op_writes_only_the_changed_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("perf.db");
        let partial = json!({ "text": "x".repeat(20_000) });
        let mut world = World::default();
        for id in 1..=3_000i64 {
            world.tasks.push(done_task(id, &partial, 1_000));
        }
        let live = done_task(3_001, &partial, 1_000);
        let live = Task {
            status: Status::Running,
            gen: 7,
            progress: 0.0,
            finished_ms: None,
            heartbeat_ms: Some(1_000),
            ..live
        };
        world.tasks.push(live);
        world.next_id = 3_002;

        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        {
            let tx = conn.unchecked_transaction().unwrap();
            save(&tx, &world).unwrap();
            tx.commit().unwrap();
        }
        drop(conn);

        let clock = Arc::new(FakeClock::new(2_000));
        let store = SqliteStore::open(&path, clock).unwrap();
        let started = Instant::now();
        let mut statements = 0;
        for step in 0..200u64 {
            let before = store.writes();
            let out = store
                .apply(Op::Progress {
                    id: 3_001,
                    gen: 7,
                    progress: step as f32 / 200.0,
                })
                .unwrap();
            assert!(out.accepted);
            let after = store.writes();
            let delta = Writes {
                inserted: after.inserted - before.inserted,
                updated: after.updated - before.updated,
                removed: after.removed - before.removed,
                statements: after.statements - before.statements,
            };
            assert_eq!(
                delta,
                Writes {
                    inserted: 0,
                    updated: 1,
                    removed: 0,
                    statements: 1,
                },
                "step {step} wrote {delta:?}"
            );
            statements += delta.statements;
        }
        println!(
            "200 progress ops over 3000 tasks with 20 KB partials: {:?}, {statements} statements",
            started.elapsed()
        );
    }
}
