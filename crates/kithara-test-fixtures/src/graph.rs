use std::collections::HashMap;

use thiserror::Error;

pub(crate) struct Node<'a> {
    pub(crate) name: &'a str,
    pub(crate) dependencies: &'a [&'a str],
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum GraphError {
    #[error("asset `{asset}` depends on missing asset `{dependency}`")]
    Missing { asset: String, dependency: String },
    #[error("asset dependency cycle: {0}")]
    Cycle(String),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Unvisited,
    Visiting,
    Visited,
}

pub(crate) fn order(nodes: &[Node<'_>]) -> Result<Vec<usize>, GraphError> {
    let by_name: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.name, index))
        .collect();
    let mut states = vec![State::Unvisited; nodes.len()];
    let mut stack = Vec::new();
    let mut ordered = Vec::with_capacity(nodes.len());

    for index in 0..nodes.len() {
        visit(
            index,
            nodes,
            &by_name,
            &mut states,
            &mut stack,
            &mut ordered,
        )?;
    }
    Ok(ordered)
}

fn visit(
    index: usize,
    nodes: &[Node<'_>],
    by_name: &HashMap<&str, usize>,
    states: &mut [State],
    stack: &mut Vec<usize>,
    ordered: &mut Vec<usize>,
) -> Result<(), GraphError> {
    match states[index] {
        State::Visited => return Ok(()),
        State::Visiting => {
            let start = stack.iter().position(|&item| item == index).unwrap_or(0);
            let mut cycle: Vec<_> = stack[start..]
                .iter()
                .map(|&item| nodes[item].name)
                .collect();
            cycle.push(nodes[index].name);
            return Err(GraphError::Cycle(cycle.join(" -> ")));
        }
        State::Unvisited => {}
    }

    states[index] = State::Visiting;
    stack.push(index);
    for dependency in nodes[index].dependencies {
        let dependency_index =
            by_name
                .get(dependency)
                .copied()
                .ok_or_else(|| GraphError::Missing {
                    asset: nodes[index].name.to_owned(),
                    dependency: (*dependency).to_owned(),
                })?;
        visit(dependency_index, nodes, by_name, states, stack, ordered)?;
    }
    stack.pop();
    states[index] = State::Visited;
    ordered.push(index);
    Ok(())
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{GraphError, Node, order};

    #[kithara::test(native, flash(false))]
    fn dependencies_precede_their_consumers() {
        let nodes = [
            Node {
                name: "analysis",
                dependencies: &["audio", "score"],
            },
            Node {
                name: "score",
                dependencies: &[],
            },
            Node {
                name: "audio",
                dependencies: &[],
            },
        ];

        let ordered = order(&nodes).expect("valid graph");
        let names: Vec<_> = ordered.iter().map(|&index| nodes[index].name).collect();

        assert_eq!(names, ["audio", "score", "analysis"]);
    }

    #[kithara::test(native, flash(false))]
    fn missing_dependency_names_both_assets() {
        let nodes = [Node {
            name: "analysis",
            dependencies: &["audio"],
        }];

        assert_eq!(
            order(&nodes),
            Err(GraphError::Missing {
                asset: "analysis".to_owned(),
                dependency: "audio".to_owned(),
            }),
        );
    }

    #[kithara::test(native, flash(false))]
    fn dependency_cycle_reports_the_closed_path() {
        let nodes = [
            Node {
                name: "audio",
                dependencies: &["analysis"],
            },
            Node {
                name: "analysis",
                dependencies: &["audio"],
            },
        ];

        assert_eq!(
            order(&nodes),
            Err(GraphError::Cycle("audio -> analysis -> audio".to_owned())),
        );
    }
}
