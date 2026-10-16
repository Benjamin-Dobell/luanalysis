use super::{NativeCheckPlan, NativePlanOptions};

fn unsplit(_: usize, _: usize) -> Vec<(u32, u32)> { Vec::new() }

#[test]
fn upstream_warmup_samples_light_files_and_grows_one_eight_sixty_four() {
    let options = NativePlanOptions::default();
    let plan = NativeCheckPlan::new(90, &|_| 100, &|_| 1, &unsplit, &options);
    assert_eq!(plan.steps.iter().map(Vec::len).collect::<Vec<_>>(), vec![1, 8, 64, 17]);
    let sample: Vec<_> = plan.steps[..3].iter().flatten().flatten().copied().collect();
    assert_eq!(sample, (0..73).map(|index| index * 90 / 73).collect::<Vec<_>>());
    assert!(!sample.contains(&5), "warmup is a representative sample rather than a source prefix");
    let mut all: Vec<_> = plan.steps.iter().flatten().flatten().copied().collect();
    all.sort_unstable();
    assert_eq!(all, (0..90).collect::<Vec<_>>());
}

#[test]
fn upstream_warmup_boundary_uses_bytes_independently_of_cost() {
    let bytes = [1, (16 << 10) + 1, 16 << 10, 50_000, 20];
    let plan = NativeCheckPlan::new(bytes.len(), &|index| bytes[index], &|_| 1, &unsplit, &NativePlanOptions::default());
    assert_eq!(plan.steps, vec![vec![vec![0]], vec![vec![2], vec![4]], vec![vec![1], vec![3]]]);
}

#[test]
fn upstream_chunks_keep_neighbors_in_one_checker_and_isolate_heavy_files() {
    let costs = [3, 4, 100, 2, 2, 2];
    let options = NativePlanOptions { warm_up_files: 0, min_tasks: 0, chunk_bytes: 5, ..Default::default() };
    let plan = NativeCheckPlan::new(costs.len(), &|_| 50_000, &|index| costs[index], &unsplit, &options);
    assert_eq!(plan.steps, vec![vec![vec![0, 1], vec![2], vec![3, 4, 5]]]);
    assert_eq!(plan.start_order(0, &|index| costs[index], 1, false), vec![1, 0, 2]);
    assert_eq!(plan.steps[0], vec![vec![0, 1], vec![2], vec![3, 4, 5]], "heavy-first start must not mutate publication order");
}

#[test]
fn upstream_min_tasks_uses_total_cost_even_when_chunk_option_is_smaller() {
    let options = NativePlanOptions { warm_up_files: 0, min_tasks: 2, chunk_bytes: 1, ..Default::default() };
    let plan = NativeCheckPlan::new(4, &|_| 50_000, &|_| 1, &unsplit, &options);
    assert_eq!(plan.steps, vec![vec![vec![0, 1], vec![2, 3]]]);
}

#[test]
fn upstream_single_thread_plan_has_no_warmup_and_preserves_execution_order() {
    let options = NativePlanOptions { one_thread_task_cost: 5, ..Default::default() };
    assert!(options.uses_one_thread_plan(1, options.one_thread_max_cost));
    assert!(!options.uses_one_thread_plan(1, options.one_thread_max_cost + 1));
    assert!(!options.uses_one_thread_plan(2, 1));
    assert!(!NativePlanOptions { tasks_on_one_thread: true, ..options }.uses_one_thread_plan(1, 1));
    let costs = [3, 4, 100, 2, 2];
    let plan = NativeCheckPlan::one_thread(costs.len(), &|index| costs[index], &options);
    assert_eq!(plan.steps, vec![vec![vec![0, 1], vec![2], vec![3, 4]]]);
    assert_eq!(plan.start_order(0, &|index| costs[index], 1, true), vec![0, 1, 2]);
}

#[test]
fn upstream_ahead_ranges_have_a_later_owning_file_task() {
    let options = NativePlanOptions { warm_up_files: 0, min_tasks: 0, chunk_bytes: 5, split_files: 4, ..Default::default() };
    let ranges = |file, parts| { assert_eq!((file, parts), (0, 4)); vec![(0, 5), (5, 10)] };
    let plan = NativeCheckPlan::new(2, &|_| 50_000, &|index| [10, 1][index], &ranges, &options);
    assert_eq!(plan.steps, vec![vec![vec![0], vec![0], vec![1]], vec![vec![0]]]);
    assert_eq!(plan.ahead_of(0), &[Some((0, 5)), Some((5, 10)), None]);
    assert!(plan.ahead_of(1).is_empty());
}

#[test]
fn empty_and_small_source_plans_cover_each_unsplit_file_exactly_once() {
    for count in [0, 1, 2, 73, 74, 150] {
        let plan = NativeCheckPlan::new(count, &|index| if index % 7 == 0 { 50_000 } else { 100 }, &|index| index + 1, &unsplit, &NativePlanOptions::default());
        let mut actual: Vec<_> = plan.steps.iter().flatten().flatten().copied().collect();
        actual.sort_unstable();
        assert_eq!(actual, (0..count).collect::<Vec<_>>());
        assert!(plan.ahead_of(0).is_empty());
    }
    assert!(NativeCheckPlan::one_thread(0, &|_| unreachable!(), &NativePlanOptions::default()).steps.is_empty());
}
