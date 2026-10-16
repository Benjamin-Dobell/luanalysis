//! Publication phases for explicit roots. Resolution discovers candidates; this plan never
//! activates them. An edge points from a consumer to one of its selected dependencies.

use luanalysis_project::{DependencyOutcome, FileId, Project};
use std::collections::HashMap;

pub(crate) fn dependency_steps(project: &Project, roots: &[FileId]) -> Vec<Vec<usize>> {
    let count = roots.len();
    if count == 0 {
        return Vec::new();
    }
    let mut edges = vec![Vec::new(); count];
    let mut first: HashMap<FileId, usize> = HashMap::new();
    for (index, &file) in roots.iter().enumerate() {
        if let Some(&prior) = first.get(&file) {
            // Duplicate explicit requests still have distinct output jobs, but they represent
            // one source and must occupy the same component/publication phase.
            edges[index].push(prior);
            edges[prior].push(index);
        } else {
            first.insert(file, index);
        }
    }
    for dependency in &project.dependencies {
        let DependencyOutcome::Resolved(target) = dependency.outcome else {
            continue;
        };
        if let (Some(&consumer), Some(&dependency)) =
            (first.get(&dependency.from), first.get(&target))
        {
            edges[consumer].push(dependency);
        }
    }
    let mut reverse = vec![Vec::new(); count];
    for (consumer, dependencies) in edges.iter_mut().enumerate() {
        dependencies.sort_unstable();
        dependencies.dedup();
        for &dependency in dependencies.iter() {
            reverse[dependency].push(consumer);
        }
    }

    // Iterative Kosaraju: neither pass consumes the native thread stack for a deep import chain.
    let mut visited = vec![false; count];
    let mut finish = Vec::with_capacity(count);
    for start in 0..count {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0)];
        while let Some((node, next)) = stack.last_mut() {
            if *next < edges[*node].len() {
                let dependency = edges[*node][*next];
                *next += 1;
                if !visited[dependency] {
                    visited[dependency] = true;
                    stack.push((dependency, 0));
                }
            } else {
                finish.push(*node);
                stack.pop();
            }
        }
    }
    let mut component_of = vec![usize::MAX; count];
    let mut members: Vec<Vec<usize>> = Vec::new();
    for &start in finish.iter().rev() {
        if component_of[start] != usize::MAX {
            continue;
        }
        let component = members.len();
        let mut nodes = Vec::new();
        let mut stack = vec![start];
        component_of[start] = component;
        while let Some(node) = stack.pop() {
            nodes.push(node);
            for &consumer in &reverse[node] {
                if component_of[consumer] == usize::MAX {
                    component_of[consumer] = component;
                    stack.push(consumer);
                }
            }
        }
        nodes.sort_unstable();
        members.push(nodes);
    }

    let mut dependencies = vec![Vec::new(); members.len()];
    for (consumer, targets) in edges.iter().enumerate() {
        let component = component_of[consumer];
        for &target in targets {
            let dependency = component_of[target];
            if dependency != component {
                dependencies[component].push(dependency);
            }
        }
    }
    let mut consumers = vec![Vec::new(); members.len()];
    let mut remaining = Vec::with_capacity(members.len());
    for (component, targets) in dependencies.iter_mut().enumerate() {
        targets.sort_unstable();
        targets.dedup();
        remaining.push(targets.len());
        for &dependency in targets.iter() {
            consumers[dependency].push(component);
        }
    }
    let mut ready: Vec<_> = (0..members.len())
        .filter(|&component| remaining[component] == 0)
        .collect();
    let mut steps = Vec::new();
    while !ready.is_empty() {
        let mut jobs: Vec<_> = ready
            .iter()
            .flat_map(|&component| members[component].iter().copied())
            .collect();
        jobs.sort_unstable();
        steps.push(jobs);
        let mut next = Vec::new();
        for component in ready {
            for &consumer in &consumers[component] {
                remaining[consumer] -= 1;
                if remaining[consumer] == 0 {
                    next.push(consumer);
                }
            }
        }
        next.sort_unstable_by_key(|&component| members[component][0]);
        ready = next;
    }
    debug_assert_eq!(steps.iter().map(Vec::len).sum::<usize>(), count);
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use luanalysis_project::{
        Dependency, ProjectConfig, RequireRequest, ResolveError, Resolver, SourceFile,
        SourceOrigin, SourceRole,
    };

    fn project(count: usize, edges: &[(usize, usize)]) -> Project {
        let resolver = Resolver::new(ProjectConfig::new(Vec::new())).unwrap();
        let mut project = Project::load(&resolver, Vec::new(), |_| Vec::new());
        for index in 0..count {
            let id = FileId {
                project: project.id,
                index: index as u32,
            };
            project.files.push(SourceFile {
                id,
                path: format!("{index}.lua").into(),
                source: String::new(),
                role: SourceRole::Implementation,
                origin: SourceOrigin::Filesystem,
            });
            project.entries.push(id);
        }
        for &(from, to) in edges {
            project.dependencies.push(Dependency {
                from: project.files[from].id,
                request: RequireRequest::new(to.to_string(), 0..0),
                outcome: DependencyOutcome::Resolved(project.files[to].id),
            });
        }
        project
    }

    #[test]
    fn dependency_chain_publishes_each_dependency_before_its_consumer() {
        let project = project(3, &[(0, 1), (1, 2)]);
        assert_eq!(
            dependency_steps(&project, &project.entries),
            vec![vec![2], vec![1], vec![0]]
        );
    }

    #[test]
    fn diamond_and_independent_roots_share_dependency_levels_in_source_order() {
        let mut project = project(5, &[(0, 1), (0, 2), (1, 3), (2, 3), (0, 1)]);
        let expected = vec![vec![3, 4], vec![1, 2], vec![0]];
        assert_eq!(dependency_steps(&project, &project.entries), expected);
        project.dependencies.reverse();
        assert_eq!(dependency_steps(&project, &project.entries), expected);
    }

    #[test]
    fn cyclic_members_remain_separate_jobs_before_an_external_consumer() {
        let project = project(4, &[(0, 1), (1, 0), (2, 0), (3, 3)]);
        assert_eq!(
            dependency_steps(&project, &project.entries),
            vec![vec![0, 1, 3], vec![2]]
        );
    }

    #[test]
    fn unselected_candidates_failed_resolutions_and_foreign_ids_do_not_add_jobs() {
        let mut project = project(4, &[(0, 1), (1, 2), (3, 0)]);
        let roots = [project.files[0].id, project.files[2].id];
        project.dependencies.push(Dependency {
            from: roots[0],
            request: RequireRequest::new("missing", 0..0),
            outcome: DependencyOutcome::Unresolved(ResolveError::InvalidModule("missing".into())),
        });
        project.dependencies.push(Dependency {
            from: roots[0],
            request: RequireRequest::new("foreign", 0..0),
            outcome: DependencyOutcome::Resolved(FileId {
                project: project.id + 1,
                index: roots[1].index,
            }),
        });
        assert_eq!(dependency_steps(&project, &roots), vec![vec![0, 1]]);
        project.dependencies.push(Dependency {
            from: roots[0],
            request: RequireRequest::new("selected", 0..0),
            outcome: DependencyOutcome::Resolved(roots[1]),
        });
        assert_eq!(dependency_steps(&project, &roots), vec![vec![1], vec![0]]);
    }

    #[test]
    fn duplicate_explicit_requests_stay_together_and_keep_every_output_index() {
        let project = project(2, &[(1, 0)]);
        let roots = [
            project.files[0].id,
            project.files[0].id,
            project.files[1].id,
        ];
        assert_eq!(
            dependency_steps(&project, &roots),
            vec![vec![0, 1], vec![2]]
        );
        assert!(dependency_steps(&project, &[]).is_empty());
    }

    #[test]
    fn deep_dependency_chain_uses_heap_worklists() {
        let count = 30_000;
        let edges: Vec<_> = (1..count).map(|node| (node - 1, node)).collect();
        let project = project(count, &edges);
        let steps = dependency_steps(&project, &project.entries);
        assert_eq!(steps.len(), count);
        assert_eq!(steps.first(), Some(&vec![count - 1]));
        assert_eq!(steps.last(), Some(&vec![0]));
    }
}
