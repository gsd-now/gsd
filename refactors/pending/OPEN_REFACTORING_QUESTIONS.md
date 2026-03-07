# Open Refactoring Questions

Issues found during coding standards pass.

---

## agent_pool

### File Length: wiring.rs (~1050 lines)

`daemon/wiring.rs` is over 1000 lines. Consider splitting:
- Pool state cleanup → own module
- Socket handling → own module or merged with existing socket code
- Payload resolution → could live with Payload type

### File Length: io.rs (~570 lines)

`daemon/io.rs` is moderately long. The `TransportMap` generic machinery might warrant its own module.

### Too Many Arguments: io_loop

`io_loop` takes 14 parameters. Signs of missing abstraction:

```rust
fn io_loop(
    fs_rx: Receiver<notify::Event>,
    socket_rx: Receiver<(String, Stream)>,
    effect_rx: Receiver<Effect>,
    events_tx: &Sender<Event>,
    worker_map: &mut WorkerMap,
    submission_map: &mut SubmissionMap,
    id_allocator: &mut IdAllocator,
    pending_responses: &mut HashSet<WorkerId>,
    kicked_paths: &mut HashSet<PathBuf>,
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
    io_config: &IoConfig,
    stop_notifier: &Arc<StopNotifier>,
) -> io::Result<()>
```

Consider grouping into `IoState` struct:

```rust
struct IoState {
    worker_map: WorkerMap,
    submission_map: SubmissionMap,
    id_allocator: IdAllocator,
    pending_responses: HashSet<WorkerId>,
    kicked_paths: HashSet<PathBuf>,
}
```

And `Paths`:

```rust
struct Paths {
    root: PathBuf,
    agents_dir: PathBuf,
    submissions_dir: PathBuf,
}
```

### Too Many Arguments: handle_fs_event

Same issue - 11 parameters could be reduced with `IoState` and `Paths` structs.
