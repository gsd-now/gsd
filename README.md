# Get SH*** Done

A task queue and multiplexer for reliably getting shell scripts done.

## Overview

GSD executes arbitrary shell scripts and deserializes their stdout. It is agnostic to what the script does - it could be a simple bash command, a Python script, or an invocation of `gsd_multiplexer submit` to dispatch work to a pool of persistent agents.

The typical workflow is:
1. Write a small Rust `main.rs` that defines your tasks and how they're processed
2. Compile it
3. Run the resulting binary to process your task queue

GSD can also be used as part of a larger program.

## Concepts

- **Queue Items** - Items that implement the `QueueItem` trait
- **Task Enum** - Wraps queue items, dispatches to underlying impl (can be derived with `#[derive(GsdTask)]`)
- **Task Results** - Deserialized from script stdout

Tasks returned from `cleanup` are added back to the queue. Only script execution is parallel; everything else is synchronous.

## Example Usage

```rust
use gsd_task_queue::{GsdTask, QueueItem, NoMoreTasks, ProcessQueueOptions, process_queue};
use serde::Deserialize;
use std::process::Command;

#[derive(GsdTask)]
enum Task {
    AnalyzeFile(AnalyzeFile),
}

struct AnalyzeFile {
    path: String,
}

struct AnalyzeFileInProgress {
    path: String,
}

struct Context {
    results: Vec<String>,
}

impl QueueItem<Context> for AnalyzeFile {
    type InProgress = AnalyzeFileInProgress;
    type Response = serde_json::Value;
    type NextTasks = NoMoreTasks;

    fn start(self, _ctx: &mut Context) -> (Self::InProgress, Command) {
        let mut cmd = Command::new("./analyze.sh");
        cmd.arg(&self.path);
        (AnalyzeFileInProgress { path: self.path }, cmd)
    }

    fn cleanup(
        in_progress: Self::InProgress,
        result: Result<Self::Response, serde_json::Error>,
        ctx: &mut Context,
    ) -> Self::NextTasks {
        match result {
            Ok(response) => {
                ctx.results.push(format!("{}: {:?}", in_progress.path, response));
            }
            Err(e) => {
                ctx.results.push(format!("{}: error - {}", in_progress.path, e));
            }
        }
        NoMoreTasks
    }
}

#[tokio::main]
async fn main() {
    let mut ctx = Context { results: vec![] };

    let queue = vec![
        Task::AnalyzeFile(AnalyzeFile { path: "src/main.rs".into() }),
        Task::AnalyzeFile(AnalyzeFile { path: "src/lib.rs".into() }),
    ];

    process_queue(queue, &mut ctx, ProcessQueueOptions { max_concurrency: 4 })
        .await
        .expect("process_queue failed");

    for result in &ctx.results {
        println!("{}", result);
    }
}
```

## Multiplexer

For long-running agent pools, use `gsd_multiplexer`:

```bash
# Start the daemon (watches <root>/tasks/ and <root>/agents/)
gsd_multiplexer daemon /path/to/root

# Submit a task (from your script)
gsd_multiplexer submit /path/to/root "task input here"
```

See `MENTAL_MODEL.md` for details on the multiplexer protocol.
