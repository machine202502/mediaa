//! Virtual catalogs of the Agent section: a tree of folders and files kept in
//! `<media>/.ai/catalogs.db`. Names, colours and the layout are virtual — they never touch the
//! files on disk. A file node either links to a file where it is (`hard = false`) or owns a
//! copy in `<media>/.ai/files` (`hard = true`), which is deleted with the node.

use anyhow::{bail, Context};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

pub type NodeId = i64;

/// Extensions catalogs take: audio and video (the caller also makes sure a video has sound,
/// see `mediaa_media::info::has_sound`).
pub fn accepted_extensions() -> Vec<&'static str> {
    [&crate::formats::VIDEO_EXTENSIONS[..], &crate::formats::AUDIO_EXTENSIONS].concat()
}

pub fn accepts(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| accepted_extensions().contains(&e.to_ascii_lowercase().as_str()))
}

/// The Agent data folder of a library.
pub fn ai_dir(media_dir: &Path) -> PathBuf {
    media_dir.join(".ai")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Folder,
    File { path: PathBuf, hard: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub name: String,
    /// `0xRRGGBB`; `None` = the theme's text colour.
    pub color: Option<u32>,
    pub kind: NodeKind,
    position: i64,
}

impl Node {
    pub fn is_folder(&self) -> bool {
        self.kind == NodeKind::Folder
    }
}

const SCHEMA: i64 = 1;

pub struct Catalogs {
    dir: PathBuf,
    db: Connection,
    /// Every node, siblings in their order.
    nodes: Vec<Node>,
}

impl Catalogs {
    /// Opens (creating if needed) the catalogs of the library whose `.ai` folder is `dir`.
    pub fn open(dir: &Path) -> anyhow::Result<Catalogs> {
        std::fs::create_dir_all(dir)?;
        let db = Connection::open(dir.join("catalogs.db"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA {
            // The tree is user data: refuse to guess with an unknown schema.
            if version != 0 {
                bail!("{} has schema {version}, expected {SCHEMA}", dir.join("catalogs.db").display());
            }
            db.execute_batch(
                "CREATE TABLE nodes (
                    id INTEGER PRIMARY KEY,
                    parent INTEGER REFERENCES nodes(id) ON DELETE CASCADE,
                    name TEXT NOT NULL,
                    color INTEGER,
                    position INTEGER NOT NULL,
                    path TEXT,
                    hard INTEGER NOT NULL DEFAULT 0
                );
                PRAGMA user_version = 1;",
            )?;
        }
        let mut c = Catalogs { dir: dir.to_path_buf(), db, nodes: Vec::new() };
        c.reload()?;
        Ok(c)
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        let mut q = self.db.prepare("SELECT id, parent, name, color, position, path, hard FROM nodes ORDER BY position, id")?;
        self.nodes = q
            .query_map([], |r| {
                let path: Option<String> = r.get(5)?;
                Ok(Node {
                    id: r.get(0)?,
                    parent: r.get(1)?,
                    name: r.get(2)?,
                    color: r.get(3)?,
                    position: r.get(4)?,
                    kind: match path {
                        Some(p) => NodeKind::File { path: p.into(), hard: r.get(6)? },
                        None => NodeKind::Folder,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(())
    }

    /// Every node (siblings in their order).
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The file a file node shows (`None` for folders).
    pub fn file_path(&self, id: NodeId) -> Option<&Path> {
        match &self.node(id)?.kind {
            NodeKind::File { path, .. } => Some(path.as_path()),
            NodeKind::Folder => None,
        }
    }

    /// Children of `parent` (`None`: the top level) in their order.
    pub fn children(&self, parent: Option<NodeId>) -> Vec<&Node> {
        self.nodes.iter().filter(|n| n.parent == parent).collect()
    }

    /// `id` and everything under it.
    pub fn subtree(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            let p = out[i];
            out.extend(self.nodes.iter().filter(|n| n.parent == Some(p)).map(|n| n.id));
            i += 1;
        }
        out
    }

    fn next_position(&self, parent: Option<NodeId>) -> i64 {
        self.children(parent).iter().map(|n| n.position + 1).max().unwrap_or(0)
    }

    fn check_parent(&self, parent: Option<NodeId>) -> anyhow::Result<()> {
        match parent.map(|p| self.node(p)) {
            Some(None) => bail!("folder {} does not exist", parent.unwrap_or_default()),
            Some(Some(n)) if !n.is_folder() => bail!("{} is not a folder", n.name),
            _ => Ok(()),
        }
    }

    pub fn create_folder(&mut self, parent: Option<NodeId>, name: &str) -> anyhow::Result<NodeId> {
        self.check_parent(parent)?;
        self.db.execute(
            "INSERT INTO nodes (parent, name, position) VALUES (?1, ?2, ?3)",
            params![parent, name.trim(), self.next_position(parent)],
        )?;
        let id = self.db.last_insert_rowid();
        self.reload()?;
        Ok(id)
    }

    /// Adds `source` under `parent`, named after it. `hard`: copy it into `.ai/files` (under a
    /// name that does not clash) and own the copy; otherwise only link to it.
    pub fn add_file(&mut self, parent: Option<NodeId>, source: &Path, hard: bool) -> anyhow::Result<NodeId> {
        self.check_parent(parent)?;
        anyhow::ensure!(source.is_file(), "{} is not a file", source.display());
        anyhow::ensure!(accepts(source), "{}: only audio and video files go into catalogs", source.display());
        let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let path = if hard {
            let files = self.dir.join("files");
            std::fs::create_dir_all(&files)?;
            let dest = unique_path(&files, &name);
            std::fs::copy(source, &dest).with_context(|| format!("copying {}", source.display()))?;
            dest
        } else {
            std::path::absolute(source)?
        };
        let inserted = self.db.execute(
            "INSERT INTO nodes (parent, name, position, path, hard) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![parent, name, self.next_position(parent), path.to_string_lossy(), hard],
        );
        if let Err(e) = inserted {
            if hard {
                let _ = std::fs::remove_file(&path);
            }
            return Err(e.into());
        }
        let id = self.db.last_insert_rowid();
        self.reload()?;
        Ok(id)
    }

    /// Virtual name only: the file on disk keeps its name.
    pub fn rename(&mut self, id: NodeId, name: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!name.trim().is_empty(), "empty name");
        self.db.execute("UPDATE nodes SET name = ?1 WHERE id = ?2", params![name.trim(), id])?;
        self.reload()
    }

    pub fn set_color(&mut self, id: NodeId, color: Option<u32>) -> anyhow::Result<()> {
        self.db.execute("UPDATE nodes SET color = ?1 WHERE id = ?2", params![color, id])?;
        self.reload()
    }

    /// Moves `id` into `parent` at `index` among its children (a folder never into itself).
    pub fn move_node(&mut self, id: NodeId, parent: Option<NodeId>, index: usize) -> anyhow::Result<()> {
        self.check_parent(parent)?;
        if let Some(p) = parent {
            anyhow::ensure!(!self.subtree(id).contains(&p), "a folder cannot go into itself");
        }
        let mut order: Vec<NodeId> = self.children(parent).iter().map(|n| n.id).filter(|&n| n != id).collect();
        order.insert(index.min(order.len()), id);
        let tx = self.db.unchecked_transaction()?;
        tx.execute("UPDATE nodes SET parent = ?1 WHERE id = ?2", params![parent, id])?;
        for (pos, n) in order.iter().enumerate() {
            tx.execute("UPDATE nodes SET position = ?1 WHERE id = ?2", params![pos as i64, n])?;
        }
        tx.commit()?;
        self.reload()
    }

    /// Deletes `id` with everything under it; copies owned by hard file nodes are deleted too.
    pub fn delete(&mut self, id: NodeId) -> anyhow::Result<()> {
        let owned: Vec<PathBuf> = self
            .subtree(id)
            .iter()
            .filter_map(|&n| match &self.node(n)?.kind {
                NodeKind::File { path, hard: true } => Some(path.clone()),
                _ => None,
            })
            .collect();
        self.db.execute("DELETE FROM nodes WHERE id = ?1", [id])?;
        for p in owned {
            if let Err(e) = std::fs::remove_file(&p) {
                log::warn!("catalogs: removing {}: {e}", p.display());
            }
        }
        self.reload()
    }

    /// Whether the node table holds `id` (for callers that keep ids around).
    pub fn exists(&self, id: NodeId) -> bool {
        self.db.query_row("SELECT 1 FROM nodes WHERE id = ?1", [id], |_| Ok(())).optional().ok().flatten().is_some()
    }
}

/// `dir/name`, or `dir/stem-2.ext`, `-3`… if taken.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem}-{n}{ext}"))).find(|p| !p.exists()).expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(c: &Catalogs, parent: Option<NodeId>) -> Vec<String> {
        c.children(parent).iter().map(|n| n.name.clone()).collect()
    }

    #[test]
    fn folders_files_and_virtual_names() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        std::fs::write(&src, b"video").unwrap();
        let ai = ai_dir(dir.path());
        let mut c = Catalogs::open(&ai).unwrap();
        let work = c.create_folder(None, " Work ").unwrap();
        let sub = c.create_folder(Some(work), "Calls").unwrap();
        let link = c.add_file(Some(sub), &src, false).unwrap();
        let copy1 = c.add_file(Some(sub), &src, true).unwrap();
        let copy2 = c.add_file(Some(sub), &src, true).unwrap();
        assert_eq!(names(&c, None), ["Work"]);
        assert_eq!(names(&c, Some(sub)), ["clip.mp4", "clip.mp4", "clip.mp4"]);
        assert_eq!(c.file_path(link).unwrap(), std::path::absolute(&src).unwrap(), "a link points at the file");
        assert_eq!(c.file_path(copy1).unwrap(), ai.join("files").join("clip.mp4"));
        assert_eq!(c.file_path(copy2).unwrap(), ai.join("files").join("clip-2.mp4"), "copies never clash");
        assert!(c.add_file(Some(link), &src, false).is_err(), "files hold no children");

        c.rename(copy1, "Meeting").unwrap();
        c.set_color(work, Some(0xff8800)).unwrap();
        assert!(src.exists() && c.file_path(copy1).unwrap().exists(), "renames are virtual");
        drop(c);
        let c = Catalogs::open(&ai).unwrap();
        assert_eq!(names(&c, Some(sub)), ["clip.mp4", "Meeting", "clip.mp4"], "kept on disk");
        assert_eq!(c.node(work).unwrap().color, Some(0xff8800));
    }

    #[test]
    fn only_audio_and_video() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Catalogs::open(&ai_dir(dir.path())).unwrap();
        for (name, ok) in [("a.MP4", true), ("b.mp3", true), ("c.pdf", false), ("d.md", false), ("e.txt", false), ("f.png", false), ("g.gif", false), ("h.exe", false)] {
            let p = dir.path().join(name);
            std::fs::write(&p, b"x").unwrap();
            assert_eq!(c.add_file(None, &p, false).is_ok(), ok, "{name}");
        }
        assert_eq!(c.children(None).len(), 2);
    }

    #[test]
    fn moving_reorders_and_never_loops() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Catalogs::open(dir.path()).unwrap();
        let a = c.create_folder(None, "A").unwrap();
        let b = c.create_folder(None, "B").unwrap();
        let x = c.create_folder(Some(a), "X").unwrap();
        c.move_node(b, None, 0).unwrap();
        assert_eq!(names(&c, None), ["B", "A"]);
        c.move_node(b, Some(a), 5).unwrap();
        assert_eq!(names(&c, Some(a)), ["X", "B"]);
        assert!(c.move_node(a, Some(x), 0).is_err(), "not into its own subfolder");
        assert!(c.move_node(a, Some(a), 0).is_err());
        c.move_node(x, None, 1).unwrap();
        assert_eq!(names(&c, None), ["A", "X"]);
    }

    #[test]
    fn deleting_removes_owned_copies_only() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.mp3");
        std::fs::write(&src, b"img").unwrap();
        let mut c = Catalogs::open(&ai_dir(dir.path())).unwrap();
        let f = c.create_folder(None, "F").unwrap();
        let inner = c.create_folder(Some(f), "In").unwrap();
        c.add_file(Some(inner), &src, false).unwrap();
        let hard = c.add_file(Some(inner), &src, true).unwrap();
        let copy = c.file_path(hard).unwrap().to_path_buf();
        c.delete(f).unwrap();
        assert!(c.children(None).is_empty() && !c.exists(inner), "the whole subtree goes");
        assert!(!copy.exists(), "owned copy deleted");
        assert!(src.exists(), "linked file untouched");
    }
}
