use crate::*;
use std::cell::Cell;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

struct Provider {
    // Deliberately not Send/Sync: the runner must construct this inside each task.
    body_reads: Rc<Cell<usize>>,
    union: bool,
}

impl Provider {
    fn new(union: bool) -> Self {
        Self {
            body_reads: Rc::new(Cell::new(0)),
            union,
        }
    }
    fn function() -> NativeFunctionKey {
        NativeFunctionKey {
            snapshot: 8301,
            index: 0,
        }
    }
    fn parameter() -> NativeTypeParameterKey {
        NativeTypeParameterKey {
            snapshot: 8301,
            index: 0,
        }
    }
}

impl NativeTypeProvider for Provider {
    fn contains_function(&self, key: NativeFunctionKey) -> bool {
        key == Self::function()
    }
    fn function_parameters(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Vec<Parameter> {
        Vec::new()
    }
    fn function_declared_return(
        &self,
        _: NativeFunctionKey,
        _: &mut dyn TypeQueries,
    ) -> Option<Type> {
        None
    }
    fn function_inferred_return(&self, _: NativeFunctionKey, _: &mut dyn TypeQueries) -> Type {
        self.body_reads.set(self.body_reads.get() + 1);
        if self.union {
            Type::Union(vec![
                Type::NativeTypeParameter(Self::parameter()),
                Type::Shape(vec![Property {
                    name: "x".into(),
                    ty: Type::Number,
                    optional: false,
                    readonly: false,
                }]),
                Type::Shape(vec![Property {
                    name: "y".into(),
                    ty: Type::String,
                    optional: false,
                    readonly: false,
                }]),
            ])
        } else {
            Type::Number
        }
    }
    fn function_outer_type_parameters(&self, _: NativeFunctionKey) -> Vec<NativeTypeParameterKey> {
        if self.union {
            vec![Self::parameter()]
        } else {
            Vec::new()
        }
    }
    fn contains_type_parameter(&self, key: NativeTypeParameterKey) -> bool {
        key == Self::parameter()
    }
    fn type_parameter_name(&self, _: NativeTypeParameterKey) -> String {
        "T".into()
    }
    fn return_cycle(&self, _: NativeFunctionKey) {
        panic!("acyclic task fixture")
    }
    fn return_type_too_deep(&self, _: NativeFunctionKey) {
        panic!("small task fixture")
    }
}

#[test]
fn shared_program_publication_reuses_lazy_returns_in_fresh_tasks() {
    for workers in [1, 2, 4] {
        let run = Engine::new().with_native_task_plan_report(
            NonZeroUsize::new(workers).unwrap(),
            &[vec![0], vec![1, 2, 3, 4]],
            |_| Provider::new(false),
            |job, provider, queries| {
                let called = queries.call(&Type::NativeFunction(Provider::function()), &[]);
                assert!(called.errors.is_empty());
                (
                    job,
                    queries.format(&called.return_type),
                    provider.body_reads.get(),
                )
            },
        );
        assert_eq!(
            run.results,
            vec![
                (0, "number".into(), 1),
                (1, "number".into(), 0),
                (2, "number".into(), 0),
                (3, "number".into(), 0),
                (4, "number".into(), 0)
            ]
        );
        assert_eq!(run.stats.tasks, 5);
        assert_eq!(run.stats.barriers, 2);
        assert!(run.stats.linked_records > 0);
        assert!(run.stats.published_entries > 0);
    }
}

#[test]
fn native_task_outputs_and_linked_generic_unions_match_across_worker_counts() {
    let mut baseline = None;
    for workers in [1, 2, 4] {
        let run = Engine::new().with_native_task_plan_report(
            NonZeroUsize::new(workers).unwrap(),
            &[vec![0, 1, 2, 3], vec![4, 5]],
            |_| Provider::new(true),
            |job, _, queries| {
                let called = queries.call(&Type::NativeFunction(Provider::function()), &[]);
                assert!(called.errors.is_empty());
                (
                    job,
                    queries.format(&called.return_type),
                    queries.is_assignable(&Type::String, &Type::Number),
                )
            },
        );
        if let Some(baseline) = &baseline {
            assert_eq!(&run.results, baseline)
        } else {
            baseline = Some(run.results);
        }
        assert!(run.stats.published_entries > 0);
    }
}

#[test]
fn task_worker_set_is_bounded_and_actually_concurrent() {
    let barrier = Arc::new(Barrier::new(4));
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let run = Engine::new().with_native_task_plan_report(
        NonZeroUsize::new(4).unwrap(),
        &[vec![0, 1, 2, 3, 4, 5, 6, 7]],
        |_| Provider::new(false),
        |job, _, queries| {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            if job < 4 {
                barrier.wait();
            }
            assert!(queries.is_assignable(&Type::Number, &Type::Number));
            active.fetch_sub(1, Ordering::SeqCst);
            job
        },
    );
    assert_eq!(run.results, (0..8).collect::<Vec<_>>());
    assert_eq!(run.stats.workers, 4);
    assert_eq!(peak.load(Ordering::SeqCst), 4);
}

#[test]
fn task_panic_preserves_payload_and_a_later_program_still_checks() {
    let panic = std::panic::catch_unwind(|| {
        Engine::new().with_native_task_plan(
            NonZeroUsize::new(2).unwrap(),
            &[vec![0, 1]],
            |_| Provider::new(false),
            |job, _, queries| {
                if job == 0 {
                    std::panic::panic_any(731_u32)
                }
                queries.is_assignable(&Type::Number, &Type::Number)
            },
        )
    })
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<u32>(), Some(&731));
    assert!(Engine::new().is_assignable(&Type::Number, &Type::Number));
}

#[test]
fn task_capabilities_cannot_be_queried_in_another_task_epoch() {
    let shared = std::sync::Mutex::new(None);
    let invalid = std::sync::atomic::AtomicBool::new(false);
    Engine::new().with_native_task_plan(
        NonZeroUsize::new(1).unwrap(),
        &[vec![0], vec![1]],
        |_| Provider::new(false),
        |job, _, queries| {
            if job == 0 {
                *shared.lock().unwrap() = Some(queries.auto_variable_constraint());
            } else {
                let prior = shared.lock().unwrap().clone().unwrap();
                let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    queries.is_auto_variable_constraint(&prior)
                }))
                .unwrap_err();
                invalid.store(caught.is::<InvalidSessionType>(), Ordering::Relaxed);
            }
        },
    );
    assert!(invalid.load(Ordering::Relaxed));
}
