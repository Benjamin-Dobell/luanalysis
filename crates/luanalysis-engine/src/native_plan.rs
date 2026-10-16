//! The pinned Bun driver's deterministic source/cost task plan.
//!
//! Reproduce with `scripts/extract-bun-plan.sh --bun /path/to/bun --write`.
//! Upstream: 408c9374b266b45fe6b8260e09be73aa82a957cd, src/sema/driver/lib.rs.
//! Source SHA-256: cc20dc8c2ac91a8dfc9c8098421f823b0a84a491d9e26819daef5f9843df504a.
//! Bun's license is retained in vendor/bun/LICENSE.md. The planner body is
//! extracted unchanged apart from public names and visibility; the source host
//! supplies Lua byte/cost estimates and supported declaration ranges.

/// Consecutive original files checked by one Checker and provider/task.
pub type NativeCheckTask = Vec<usize>;
/// An upstream declaration statement-range task, if the source host supports it.
pub type NativeCheckAhead = Option<(u32, u32)>;

/// The upstream options read by this pure planner or its caller's plan choice.
/// Other driver options are deliberately outside this extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativePlanOptions {
    pub step_growth: usize,
    pub warm_up_files: usize,
    pub warm_up_max_bytes: usize,
    pub chunk_bytes: usize,
    pub min_tasks: usize,
    pub type_node_cost: usize,
    pub split_files: u32,
    pub tasks_on_one_thread: bool,
    pub one_thread_task_cost: usize,
    pub one_thread_max_cost: usize,
}

impl Default for NativePlanOptions {
    fn default() -> Self {
        Self {
            step_growth: 8,
            warm_up_files: 1 + 8 + 64,
            warm_up_max_bytes: 16 << 10,
            chunk_bytes: 64 << 10,
            min_tasks: 128,
            type_node_cost: 1,
            split_files: 64,
            tasks_on_one_thread: false,
            one_thread_task_cost: 256 << 10,
            one_thread_max_cost: 1 << 20,
        }
    }
}

/// Task/publication order depends on source metadata, never completion time or
/// worker count. `one_thread` deliberately uses Bun's different serial plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCheckPlan {
    pub steps: Vec<Vec<NativeCheckTask>>,
    ahead: Vec<NativeCheckAhead>,
}

impl NativeCheckPlan {
    /// The tasks of one thread for `files`, which are in program order like those of
    /// `tsc --singleThreaded`: as few as `one_thread_task_cost` allows. Most programs are one task.
    ///
    /// Every task computes its own copy of what it shares with the other tasks of its step. That
    /// occupies threads that have nothing else to do, and one thread has: with the plan of several
    /// threads it is a fifth to a half of all the work. But a task keeps what it has computed until
    /// it ends, so its size is what the memory grows by.
    fn of_one_thread(
        files: Vec<usize>,
        size_of: &dyn Fn(usize) -> usize,
        options: &NativePlanOptions,
    ) -> Vec<NativeCheckTask> {
        let options = NativePlanOptions {
            min_tasks: 0,
            chunk_bytes: options.one_thread_task_cost,
            ..*options
        };
        NativeCheckPlan::cut(files, size_of, &options)
    }

    /// `count`: the number of files to check. `size_of(i)`: the source size of file `i` in bytes.
    ///
    /// No order among the tasks is needed: what they observe of each other is published, or each
    /// computes its own copy.
    pub fn new(
        count: usize,
        bytes_of: &dyn Fn(usize) -> usize,
        size_of: &dyn Fn(usize) -> usize,
        ranges_of: &dyn Fn(usize, usize) -> Vec<(u32, u32)>,
        options: &NativePlanOptions,
    ) -> NativeCheckPlan {
        assert!(options.step_growth >= 1);
        let mut steps: Vec<Vec<NativeCheckTask>> = Vec::new();
        // The warm-up fills the published state with what most tasks need. Until that is published,
        // each task of a step computes its own copy. The steps should be short, so they consist of
        // light files. The files are a sample of the whole program, because its first files are not
        // representative of the rest.
        let is_heavy = |file: usize| bytes_of(file) > options.warm_up_max_bytes;
        let light: Vec<usize> = (0..count).filter(|&file| !is_heavy(file)).collect();
        let expected = options.warm_up_files.min(light.len());
        let warm_up = (0..expected).map(|i| light[i * light.len() / expected]);
        let warm_up: Vec<usize> = warm_up.collect();
        let (mut tasks, mut limit) = (warm_up.iter().map(|&file| vec![file]).peekable(), 1usize);
        while tasks.peek().is_some() {
            steps.push(tasks.by_ref().take(limit).collect());
            limit = limit.saturating_mul(options.step_growth);
        }
        // One step with all other files. The heaviest file of a program takes about as long as a
        // thread's share of the whole check, so there is one step in which heavy files run, and
        // they are started first.
        let rest = (0..count).filter(|file| warm_up.binary_search(file).is_err());
        let chunks = NativeCheckPlan::cut(rest.collect(), size_of, options);
        // `split_files`. `ranges_of(file, parts)`: see there.
        let cost: usize = chunks.iter().flatten().map(|&file| size_of(file)).sum();
        let parts_of = |file: usize| (size_of(file) * options.split_files as usize).div_ceil(cost);
        let (mut tasks, mut ahead, mut split) = (Vec::new(), Vec::new(), Vec::new());
        for chunk in chunks {
            let ranges = match chunk[..] {
                [file] if parts_of(file) > 1 => ranges_of(file, parts_of(file)),
                _ => Vec::new(),
            };
            if ranges.is_empty() {
                tasks.push(chunk);
                ahead.push(None);
                continue;
            }
            for range in ranges {
                tasks.push(chunk.clone());
                ahead.push(Some(range));
            }
            split.push(chunk);
        }
        if !tasks.is_empty() {
            steps.push(tasks);
        }
        if split.is_empty() {
            ahead.clear();
        } else {
            steps.push(split);
        }
        NativeCheckPlan { steps, ahead }
    }

    /// `ahead`, if `number` is the step whose tasks it describes.
    pub fn ahead_of(&self, number: usize) -> &[NativeCheckAhead] {
        if number + 2 == self.steps.len() {
            &self.ahead
        } else {
            &[]
        }
    }

    /// The tasks of one step for `rest`, which is in program order.
    pub fn cut(rest: Vec<usize>, size_of: &dyn Fn(usize) -> usize, options: &NativePlanOptions) -> Vec<NativeCheckTask> {
        let bytes: usize = rest.iter().map(|&file| size_of(file)).sum();
        // About `min_tasks` tasks, whatever the size of the program: enough for any number of threads, and every further task computes
        // its own copy of what it shares with the others.
        let chunk_bytes = match options.min_tasks {
            0 => options.chunk_bytes,
            tasks => (bytes / tasks).max(1),
        };
        let is_heavy = |file: usize| size_of(file) >= chunk_bytes;
        // The files of a task share a buffer, and neighbours in program order use the same types.
        let mut chunks: Vec<NativeCheckTask> = Vec::new();
        // The source size of the last chunk, if it consists of light files.
        let mut bytes_of_last = None;
        for file in rest {
            match bytes_of_last {
                _ if is_heavy(file) => {
                    chunks.push(vec![file]);
                    bytes_of_last = None;
                }
                Some(so_far) if so_far < chunk_bytes => {
                    chunks.last_mut().unwrap().push(file);
                    bytes_of_last = Some(so_far + size_of(file));
                }
                _ => {
                    chunks.push(vec![file]);
                    bytes_of_last = Some(size_of(file));
                }
            }
        }
        chunks
    }
}

impl NativePlanOptions {
    /// The exact choice made by the upstream driver before constructing a plan.
    /// `total_cost` also includes any sources that will only be emitted there.
    pub fn uses_one_thread_plan(&self, threads: usize, total_cost: usize) -> bool {
        threads == 1 && !self.tasks_on_one_thread && total_cost <= self.one_thread_max_cost
    }
}

impl NativeCheckPlan {
    /// The upstream one-thread plan, using one Checker per returned task.
    pub fn one_thread(count: usize, cost_of: &dyn Fn(usize) -> usize, options: &NativePlanOptions) -> Self {
        let tasks = Self::of_one_thread((0..count).collect(), cost_of, options);
        Self { steps: if tasks.is_empty() { Vec::new() } else { vec![tasks] }, ahead: Vec::new() }
    }

    /// Original task indices in the upstream execution order. Publication MUST
    /// still use the original indices in `steps`, regardless of completion order.
    pub fn start_order(&self, number: usize, cost_of: &dyn Fn(usize) -> usize, order: u32, is_one_thread: bool) -> Vec<usize> {
        let step = &self.steps[number];
        let tasks = step.len();
        let weight_of = |i: usize| step[i].iter().map(|&it| cost_of(it)).sum::<usize>();
        // The largest first, so that no thread begins it when the others are nearly done.
        let mut start_order: Vec<usize> = (0..tasks).collect();
        match order {
            1 if is_one_thread => {}
            1 => start_order.sort_by_key(|&i| std::cmp::Reverse(weight_of(i))),
            order => start_order.sort_by_key(|&i| (i as u32 + 1).wrapping_mul(order)),
        }
        start_order
    }
}
