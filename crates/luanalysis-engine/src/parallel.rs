//! One shared native program, fresh task state, and deterministic publication barriers.

use super::{
    Engine, NativeTypeProvider, ProviderAdapter, SessionQueries, TypeHandles, native, with_program,
};
use bun_sema::check::{Program, task::Finished};
use std::any::Any;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Mutex, mpsc};

/// Counts from actual native task/barrier execution, not an estimate of cache reuse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeTaskRunStats {
    pub workers: usize,
    pub tasks: usize,
    pub retries: usize,
    pub barriers: usize,
    pub linked_records: usize,
    pub joined_records: usize,
    pub published_entries: u64,
}

#[derive(Debug)]
pub struct NativeTaskRun<R> {
    /// Flattened step/task order, independent of the order in which workers finish.
    pub results: Vec<R>,
    pub stats: NativeTaskRunStats,
}

#[derive(Clone, Copy)]
struct Command {
    step: u32,
    index: u32,
    slot: usize,
    job: usize,
}

struct Completion<'s, R> {
    slot: usize,
    result: R,
    finished: Finished<'s>,
}

type Panic = Box<dyn Any + Send + 'static>;

impl Engine {
    /// Runs a deterministic task plan with one shared Session/Program and a bounded worker set.
    ///
    /// `steps` contains source-defined job IDs. Jobs may be root batches rather than files.
    /// The plan must be the same for every worker count used in a differential check. Each job
    /// gets a fresh provider, checker and handle epoch. The factory runs inside its worker, so
    /// `P` may contain Rc/RefCell and need not implement Send or Sync. The factory and immutable
    /// source snapshot it closes over must be Sync; never share worker-local provider state.
    /// A fresh metadata-only provider is also constructed at each barrier for native union
    /// ordering. The factory must be side-effect-free and expose all source metadata to each job.
    ///
    /// Render all report types in `work`, before it returns. SessionType, pack and context
    /// capabilities belong to that task and cannot be queried by another task or after the run.
    /// At each barrier native validation, linking and publication run in plan order. Native
    /// source caches that are currently Checker-local are recomputed in later tasks; this API
    /// does not claim to publish those caches or to provide cross-edit incremental reuse.
    pub fn with_native_task_plan<P: NativeTypeProvider, R: Send>(
        &self,
        workers: NonZeroUsize,
        steps: &[Vec<usize>],
        factory: impl Fn(usize) -> P + Sync,
        work: impl Fn(usize, &P, &mut dyn super::TypeQueries) -> R + Sync,
    ) -> Vec<R> {
        self.with_native_task_plan_report(workers, steps, factory, work)
            .results
    }

    pub fn with_native_task_plan_report<P: NativeTypeProvider, R: Send>(
        &self,
        workers: NonZeroUsize,
        steps: &[Vec<usize>],
        factory: impl Fn(usize) -> P + Sync,
        work: impl Fn(usize, &P, &mut dyn super::TypeQueries) -> R + Sync,
    ) -> NativeTaskRun<R> {
        let tasks: usize = steps.iter().map(Vec::len).sum();
        if tasks == 0 {
            return NativeTaskRun {
                results: Vec::new(),
                stats: NativeTaskRunStats::default(),
            };
        }
        let workers = workers.get().min(steps.iter().map(Vec::len).max().unwrap());
        with_program(|program, _coordinator_handles| run(program, workers, steps, &factory, &work))
    }
}

fn run<'s, P: NativeTypeProvider, R: Send>(
    program: &Program<'s>,
    workers: usize,
    steps: &[Vec<usize>],
    factory: &(impl Fn(usize) -> P + Sync),
    work: &(impl Fn(usize, &P, &mut dyn super::TypeQueries) -> R + Sync),
) -> NativeTaskRun<R> {
    std::thread::scope(|scope| {
        let (commands, receive_commands) = mpsc::channel::<Command>();
        let receive_commands = Arc::new(Mutex::new(receive_commands));
        let (completions, receive_completions) =
            mpsc::channel::<Result<Completion<'s, R>, Panic>>();
        let mut threads = Vec::with_capacity(workers);
        for worker in 0..workers {
            let receive_commands = Arc::clone(&receive_commands);
            let completions = completions.clone();
            threads.push(
                std::thread::Builder::new()
                    .name(format!("luanalysis-type-task-{worker}"))
                    .stack_size(bun_threading::thread_pool::DEFAULT_THREAD_STACK_SIZE as usize)
                    .spawn_scoped(scope, move || {
                        native::set_stack_size(
                            bun_threading::thread_pool::DEFAULT_THREAD_STACK_SIZE as usize
                                - (256 << 10),
                        );
                        loop {
                            // Only receiving a command is serialized. No source callback runs under
                            // this lock, and each task owns its mutable checker/provider/handles.
                            let command =
                                receive_commands.lock().expect("native task queue").recv();
                            let Ok(command) = command else { break };
                            let outcome = catch_unwind(AssertUnwindSafe(|| {
                                let handles = TypeHandles::new();
                                let provider = factory(command.job);
                                let adapter = ProviderAdapter {
                                    provider: &provider,
                                    handles: &handles,
                                };
                                let mut checker = program.checker_with_native_provider(&adapter);
                                checker.set_stack_limit(
                                    bun_threading::thread_pool::DEFAULT_THREAD_STACK_SIZE as usize
                                        - (256 << 10),
                                );
                                checker.set_native_type_declaration_provider(&adapter);
                                checker.set_native_globals_provider(&adapter);
                                checker.set_native_project_provider(&adapter);
                                checker.enable_native_arrays();
                                // No native IDs may be created before this resets the task stores.
                                checker.begin_task(command.step, command.index, true);
                                let result = work(
                                    command.job,
                                    &provider,
                                    &mut SessionQueries {
                                        checker: &mut checker,
                                        handles: &handles,
                                    },
                                );
                                let finished = checker.end_task();
                                Completion {
                                    slot: command.slot,
                                    result,
                                    finished,
                                }
                            }));
                            if completions.send(outcome).is_err() {
                                break;
                            }
                        }
                    })
                    .expect("create native type task worker"),
            );
        }
        drop(completions);

        // Ensure every worker stops before a callback panic reaches Program::release.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut stats = NativeTaskRunStats {
                workers,
                ..NativeTaskRunStats::default()
            };
            let mut results = Vec::new();
            let serial = |count: usize, work: &(dyn Fn(usize) + Sync)| (0..count).for_each(work);
            for (step_number, jobs) in steps.iter().enumerate() {
                if jobs.is_empty() {
                    continue;
                }
                program.compare_listings();
                let mut outputs: Vec<Option<R>> = (0..jobs.len()).map(|_| None).collect();
                let mut pending: Vec<usize> = (0..jobs.len()).collect();
                let mut prior_retry_count = None;
                while !pending.is_empty() {
                    for (index, &slot) in pending.iter().enumerate() {
                        commands
                            .send(Command {
                                step: u32::try_from(step_number)
                                    .expect("native task step capacity"),
                                index: u32::try_from(index).expect("native task index capacity"),
                                slot,
                                job: jobs[slot],
                            })
                            .expect("native task workers stopped");
                    }
                    let mut completed = Vec::with_capacity(pending.len());
                    let mut panic = None;
                    for _ in &pending {
                        match receive_completions
                            .recv()
                            .expect("native task worker lost a result")
                        {
                            Ok(completion) => completed.push(completion),
                            Err(error) => {
                                if panic.is_none() {
                                    panic = Some(error)
                                }
                            }
                        }
                    }
                    if let Some(error) = panic {
                        resume_unwind(error)
                    }
                    completed.sort_by_key(|completion| completion.finished.index);
                    let (payloads, mut finished): (Vec<_>, Vec<_>) = completed
                        .into_iter()
                        .map(|completion| {
                            ((completion.slot, completion.result), completion.finished)
                        })
                        .unzip();
                    let invalid = program.validate(&finished, true);
                    let mut next = Vec::new();
                    let mut accepted = Vec::new();
                    for (((slot, result), finished), invalid) in
                        payloads.into_iter().zip(finished.drain(..)).zip(invalid)
                    {
                        stats.tasks += 1;
                        if invalid {
                            stats.retries += 1;
                            next.push(slot);
                        } else {
                            outputs[slot] = Some(result);
                            accepted.push(finished);
                        }
                    }
                    // Sorting native parameters needs source metadata. This checker never
                    // enters a task or runs source body queries, and is dropped at the barrier.
                    let metadata_handles = TypeHandles::new();
                    let metadata_provider = factory(jobs[pending[0]]);
                    let metadata_adapter = ProviderAdapter {
                        provider: &metadata_provider,
                        handles: &metadata_handles,
                    };
                    let mut metadata_checker =
                        program.checker_with_native_provider(&metadata_adapter);
                    metadata_checker.enable_native_arrays();
                    metadata_checker.set_native_type_declaration_provider(&metadata_adapter);
                    metadata_checker.set_native_globals_provider(&metadata_adapter);
                    metadata_checker.set_native_project_provider(&metadata_adapter);
                    let linked = program.link_with_sort(&mut accepted, &serial, &|types| {
                        metadata_checker.sort_types(types)
                    });
                    let published = program.publish(&mut accepted, &serial, false);
                    stats.barriers += 1;
                    stats.linked_records += linked.linked.iter().sum::<usize>();
                    stats.joined_records += linked.joined.iter().sum::<usize>();
                    stats.published_entries += published.published;
                    if !next.is_empty() {
                        if prior_retry_count.is_some_and(|prior| next.len() >= prior) {
                            program.stop_comparing_listings();
                        }
                        prior_retry_count = Some(next.len());
                    }
                    pending = next;
                }
                results.extend(
                    outputs
                        .into_iter()
                        .map(|result| result.expect("native task result")),
                );
            }
            NativeTaskRun { results, stats }
        }));
        drop(commands);
        for thread in threads {
            if let Err(error) = thread.join() {
                if outcome.is_ok() {
                    resume_unwind(error)
                }
            }
        }
        outcome.unwrap_or_else(|error| resume_unwind(error))
    })
}
