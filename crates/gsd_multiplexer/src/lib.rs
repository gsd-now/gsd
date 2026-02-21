use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{fs, io};

pub const TASKS_DIR: &str = "tasks";
pub const AGENTS_DIR: &str = "agents";
pub const LOCK_FILE: &str = "daemon.lock";

pub fn submit(root: impl AsRef<Path>, input: &str) -> io::Result<String> {
    let root = root.as_ref();
    let tasks_folder = root.join(TASKS_DIR);
    fs::create_dir_all(&tasks_folder)?;

    let hash = generate_hash();
    let input_path = tasks_folder.join(format!("{}.input", hash));
    let output_path = tasks_folder.join(format!("{}.output", hash));

    fs::write(&input_path, input)?;

    while !output_path.exists() {
        thread::sleep(Duration::from_millis(100));
    }

    let output = fs::read_to_string(&output_path)?;
    fs::remove_file(&output_path)?;

    Ok(output)
}

fn generate_hash() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", now)
}

pub struct Multiplexer {
    root: PathBuf,
    tasks_folder: PathBuf,
    agents_folder: PathBuf,
    lock_path: PathBuf,
    agents: HashMap<String, AgentState>,
    pending_tasks: VecDeque<PendingTask>,
}

struct PendingTask {
    hash: String,
    content: String,
}

#[derive(Debug)]
enum AgentState {
    Available,
    Busy { task_hash: String },
}

impl Multiplexer {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let tasks_folder = root.join(TASKS_DIR);
        let agents_folder = root.join(AGENTS_DIR);
        let lock_path = root.join(LOCK_FILE);

        fs::create_dir_all(&root)?;
        fs::create_dir_all(&tasks_folder)?;
        fs::create_dir_all(&agents_folder)?;

        if lock_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("Daemon already running (lock file exists: {})", lock_path.display()),
            ));
        }

        fs::write(&lock_path, std::process::id().to_string())?;

        Ok(Self {
            root,
            tasks_folder,
            agents_folder,
            lock_path,
            agents: HashMap::new(),
            pending_tasks: VecDeque::new(),
        })
    }

    pub fn register_agent(&mut self, agent_id: &str) -> io::Result<()> {
        let agent_folder = self.agents_folder.join(agent_id);
        fs::create_dir_all(&agent_folder)?;
        self.agents.insert(agent_id.to_string(), AgentState::Available);
        Ok(())
    }

    pub fn run(&mut self) -> io::Result<()> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event);
                }
            },
            notify::Config::default(),
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        watcher
            .watch(&self.tasks_folder, RecursiveMode::NonRecursive)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        watcher
            .watch(&self.agents_folder, RecursiveMode::Recursive)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        loop {
            match rx.recv() {
                Ok(event) => self.handle_event(event)?,
                Err(_) => break,
            }
        }

        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> io::Result<()> {
        for path in event.paths {
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if filename.ends_with(".input") && path.starts_with(&self.tasks_folder) {
                    self.handle_new_task(&path)?;
                } else if filename == "output" && path.starts_with(&self.agents_folder) {
                    self.handle_agent_output(&path)?;
                }
            }
        }
        self.try_dispatch_tasks()?;
        Ok(())
    }

    fn handle_new_task(&mut self, path: &Path) -> io::Result<()> {
        let filename = path.file_name().unwrap().to_str().unwrap();
        let hash = filename.trim_end_matches(".input").to_string();
        let content = fs::read_to_string(path)?;
        fs::remove_file(path)?;

        self.pending_tasks.push_back(PendingTask { hash, content });
        Ok(())
    }

    fn handle_agent_output(&mut self, path: &Path) -> io::Result<()> {
        let agent_folder = path.parent().unwrap();
        let agent_id = agent_folder.file_name().unwrap().to_str().unwrap();

        if let Some(AgentState::Busy { task_hash }) = self.agents.get(agent_id) {
            let output = fs::read_to_string(path)?;
            fs::remove_file(path)?;

            let output_path = self.tasks_folder.join(format!("{}.output", task_hash));
            fs::write(output_path, output)?;

            self.agents.insert(agent_id.to_string(), AgentState::Available);
        }

        Ok(())
    }

    fn try_dispatch_tasks(&mut self) -> io::Result<()> {
        let available_agent = self
            .agents
            .iter()
            .find(|(_, state)| matches!(state, AgentState::Available))
            .map(|(id, _)| id.clone());

        if let Some(agent_id) = available_agent {
            if let Some(task) = self.pending_tasks.pop_front() {
                let input_path = self.agents_folder.join(&agent_id).join("input");
                fs::write(&input_path, &task.content)?;
                self.agents.insert(
                    agent_id,
                    AgentState::Busy {
                        task_hash: task.hash,
                    },
                );
            }
        }

        Ok(())
    }
}

impl Drop for Multiplexer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}
