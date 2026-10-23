//! Task storage: an in-memory collection that loads from and saves to JSON.

use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use model::{QuickAdd, Status, Task, TaskId};
use serde::{Deserialize, Serialize};

/// Errors from loading, saving or editing a store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read or write the store: {0}")]
    Io(#[from] std::io::Error),
    #[error("the store file is not valid: {0}")]
    Format(#[from] serde_json::Error),
    #[error("no task {0}")]
    NotFound(TaskId),
    #[error("store format version {0} is newer than this program understands")]
    TooNew(u32),
}

/// On-disk format version written by this program.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    next_id: u32,
    tasks: Vec<Task>,
}

/// All tasks, ordered by id.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Store {
    tasks: Vec<Task>,
    next_id: u32,
}

impl Store {
    pub fn new() -> Store {
        Store {
            tasks: Vec::new(),
            next_id: 1,
        }
    }

    /// Loads a store, or returns an empty one when the file does not exist.
    pub fn load(path: &Path) -> Result<Store, StoreError> {
        if !path.exists() {
            return Ok(Store::new());
        }
        Store::from_json(&fs::read_to_string(path)?)
    }

    /// Writes the store atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, self.to_json()?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn from_json(text: &str) -> Result<Store, StoreError> {
        let file: FileFormat = serde_json::from_str(text)?;
        if file.version > FORMAT_VERSION {
            return Err(StoreError::TooNew(file.version));
        }
        let mut tasks = file.tasks;
        tasks.sort_by_key(|t| t.id);
        let max_id = tasks.iter().map(|t| t.id.0).max().unwrap_or(0);
        Ok(Store {
            tasks,
            next_id: file.next_id.max(max_id + 1),
        })
    }

    pub fn to_json(&self) -> Result<String, StoreError> {
        let file = FileFormat {
            version: FORMAT_VERSION,
            next_id: self.next_id,
            tasks: self.tasks.clone(),
        };
        Ok(serde_json::to_string_pretty(&file)?)
    }

    /// Adds a task from quick-add input and returns its id.
    pub fn add(&mut self, input: QuickAdd, today: NaiveDate) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        self.tasks.push(Task::from_quick_add(id, input, today));
        id
    }

    pub fn get(&self, id: TaskId) -> Result<&Task, StoreError> {
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or(StoreError::NotFound(id))
    }

    pub fn get_mut(&mut self, id: TaskId) -> Result<&mut Task, StoreError> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(StoreError::NotFound(id))
    }

    /// Sets the status of a task.
    pub fn set_status(&mut self, id: TaskId, status: Status) -> Result<(), StoreError> {
        self.get_mut(id)?.status = status;
        Ok(())
    }

    /// Removes a task and returns it.
    pub fn remove(&mut self, id: TaskId) -> Result<Task, StoreError> {
        let index = self
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or(StoreError::NotFound(id))?;
        Ok(self.tasks.remove(index))
    }

    /// Removes every finished task and returns how many were removed.
    pub fn purge_done(&mut self) -> usize {
        let done: Vec<&Task> = self
            .tasks
            .iter()
            .filter(|t| t.status.is_finished())
            .collect();
        for task in &done {
            self.remove(task.id).ok();
        }
        done.len()
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{parse_date, parse_quick_add};

    fn today() -> NaiveDate {
        parse_date("2026-10-23").unwrap()
    }

    fn sample() -> Store {
        let mut s = Store::new();
        s.add(
            parse_quick_add("write report +work !high").unwrap(),
            today(),
        );
        s.add(parse_quick_add("water plants +home").unwrap(), today());
        s
    }

    #[test]
    fn ids_increase() {
        let mut s = sample();
        let id = s.add(parse_quick_add("third").unwrap(), today());
        assert_eq!(id, TaskId(3));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn json_round_trip() {
        let s = sample();
        let back = Store::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn next_id_survives_removals() {
        let mut s = sample();
        s.remove(TaskId(2)).unwrap();
        let mut back = Store::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(
            back.add(parse_quick_add("new").unwrap(), today()),
            TaskId(3)
        );
    }

    #[test]
    fn rejects_newer_format() {
        let err = Store::from_json(r#"{"version": 9, "next_id": 1, "tasks": []}"#).unwrap_err();
        assert!(matches!(err, StoreError::TooNew(9)));
    }

    #[test]
    fn status_and_purge() {
        let mut s = sample();
        s.set_status(TaskId(1), Status::Done).unwrap();
        assert!(matches!(
            s.set_status(TaskId(9), Status::Done),
            Err(StoreError::NotFound(TaskId(9)))
        ));
        assert_eq!(s.purge_done(), 1);
        assert_eq!(s.tasks()[0].title, "water plants");
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("tasktrack-store-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tasks.json");
        let s = sample();
        s.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap(), s);
        assert!(Store::load(&dir.join("missing.json")).unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
