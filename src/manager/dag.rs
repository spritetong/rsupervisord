// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

use crate::error::ProgramError;
use crate::program::config::ProgramConfig;
use petgraph::graph::DiGraph;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// Startup layers ordered from shallowest to deepest; programs within a layer have no inter-dependencies
    /// and can be started concurrently, sorted by priority (lowest number first).
    pub start_layers: Vec<Vec<String>>,
    /// Shutdown layers ordered from deepest to shallowest (reverse topological); programs within a layer
    /// are stopped in reverse priority order (highest number first).
    pub stop_layers: Vec<Vec<String>>,
}

impl DependencyGraph {
    /// Builds the DAG, checks for circular dependencies, and computes startup & shutdown execution layers.
    pub fn build(programs: &HashMap<String, ProgramConfig>) -> Result<Self, ProgramError> {
        let mut graph = DiGraph::<String, ()>::new();
        let mut node_indices = HashMap::new();

        // 1. Register all program nodes
        for name in programs.keys() {
            let idx = graph.add_node(name.clone());
            node_indices.insert(name.clone(), idx);
        }

        // 2. Add dependency edges: dependency -> dependent (prerequisite points to dependent)
        for (name, cfg) in programs {
            let to_idx = *node_indices.get(name).ok_or_else(|| {
                ProgramError::ConfigError(format!("Internal error: missing node for {}", name))
            })?;

            for dep in &cfg.depends_on {
                let from_idx = node_indices.get(dep).ok_or_else(|| {
                    ProgramError::ConfigError(format!(
                        "Program '{}' depends on unknown program '{}'",
                        name, dep
                    ))
                })?;

                graph.add_edge(*from_idx, to_idx, ());
            }
        }

        // 3. Cycle detection using petgraph topological search
        if petgraph::algo::is_cyclic_directed(&graph) {
            return Err(ProgramError::ConfigError(
                "Circular dependency detected in program dependencies DAG".to_string(),
            ));
        }

        // 4. Compute startup layers using Kahn's algorithm (layered in-degree calculation)
        let mut in_degrees: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();

        for name in programs.keys() {
            in_degrees.insert(name.clone(), 0);
            adj.insert(name.clone(), Vec::new());
        }

        for (name, cfg) in programs {
            for dep in &cfg.depends_on {
                *in_degrees.entry(name.clone()).or_insert(0) += 1;
                adj.entry(dep.clone()).or_default().push(name.clone());
            }
        }

        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        for (name, deg) in &in_degrees {
            if *deg == 0 {
                queue.push_back((name.clone(), 0));
            }
        }

        let mut layers_map: HashMap<usize, Vec<String>> = HashMap::new();
        let mut visited_count = 0;

        while let Some((node, layer)) = queue.pop_front() {
            visited_count += 1;
            layers_map.entry(layer).or_default().push(node.clone());

            if let Some(neighbors) = adj.get(&node) {
                for neighbor in neighbors {
                    if let Some(deg) = in_degrees.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back((neighbor.clone(), layer + 1));
                        }
                    }
                }
            }
        }

        if visited_count != programs.len() {
            return Err(ProgramError::ConfigError(
                "Circular dependency detected while calculating execution layers".to_string(),
            ));
        }

        // 5. Sort programs within each layer by priority (lower number starts first: 0 before 10 before 99)
        let mut max_layer = 0;
        for &l in layers_map.keys() {
            if l > max_layer {
                max_layer = l;
            }
        }

        let mut start_layers = Vec::new();
        if !programs.is_empty() {
            for l in 0..=max_layer {
                if let Some(mut progs) = layers_map.remove(&l) {
                    progs.sort_by(|a, b| {
                        let prio_a = programs.get(a).map(|c| c.priority).unwrap_or(50);
                        let prio_b = programs.get(b).map(|c| c.priority).unwrap_or(50);
                        prio_a.cmp(&prio_b).then_with(|| a.cmp(b))
                    });
                    start_layers.push(progs);
                }
            }
        }

        // 6. Shutdown layers: reverse topology (deepest dependents stopped first, higher priority numbers stopped first)
        let mut stop_layers = start_layers.clone();
        stop_layers.reverse();
        for layer in &mut stop_layers {
            layer.sort_by(|a, b| {
                let prio_a = programs.get(a).map(|c| c.priority).unwrap_or(50);
                let prio_b = programs.get(b).map(|c| c.priority).unwrap_or(50);
                prio_b.cmp(&prio_a).then_with(|| a.cmp(b))
            });
        }

        Ok(Self {
            start_layers,
            stop_layers,
        })
    }

    /// Recursively retrieves all downstream dependents of a program.
    pub fn get_dependents(
        programs: &HashMap<String, ProgramConfig>,
        target: &str,
    ) -> HashSet<String> {
        let mut dependents = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(target.to_string());

        while let Some(current) = queue.pop_front() {
            for (name, cfg) in programs {
                if cfg.depends_on.contains(&current) && !dependents.contains(name) {
                    dependents.insert(name.clone());
                    queue.push_back(name.clone());
                }
            }
        }

        dependents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_layers_and_priorities() {
        let mut map = HashMap::new();

        let mut db = ProgramConfig::new("db", "mysqld");
        db.priority = 10;
        map.insert("db".to_string(), db);

        let mut redis = ProgramConfig::new("redis", "redis-server");
        redis.priority = 20;
        map.insert("redis".to_string(), redis);

        let mut api = ProgramConfig::new("api", "server");
        api.priority = 30;
        api.depends_on = vec!["db".to_string(), "redis".to_string()];
        map.insert("api".to_string(), api);

        let mut web = ProgramConfig::new("web", "nginx");
        web.priority = 50;
        web.depends_on = vec!["api".to_string()];
        map.insert("web".to_string(), web);

        let dag = DependencyGraph::build(&map).expect("DAG build failed");

        // 启动层级: Layer 0: [db, redis], Layer 1: [api], Layer 2: [web]
        assert_eq!(dag.start_layers.len(), 3);
        assert_eq!(
            dag.start_layers[0],
            vec!["db".to_string(), "redis".to_string()]
        );
        assert_eq!(dag.start_layers[1], vec!["api".to_string()]);
        assert_eq!(dag.start_layers[2], vec!["web".to_string()]);

        // 关机层级: Layer 0: [web], Layer 1: [api], Layer 2: [redis, db] (redis priority 20 > db priority 10)
        assert_eq!(dag.stop_layers.len(), 3);
        assert_eq!(dag.stop_layers[0], vec!["web".to_string()]);
        assert_eq!(dag.stop_layers[1], vec!["api".to_string()]);
        assert_eq!(
            dag.stop_layers[2],
            vec!["redis".to_string(), "db".to_string()]
        );

        // 级联依赖查询: db 的下游依赖为 api 与 web
        let deps = DependencyGraph::get_dependents(&map, "db");
        assert!(deps.contains("api"));
        assert!(deps.contains("web"));
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_dag_cycle_detection() {
        let mut map = HashMap::new();

        let mut a = ProgramConfig::new("a", "echo a");
        a.depends_on = vec!["b".to_string()];
        map.insert("a".to_string(), a);

        let mut b = ProgramConfig::new("b", "echo b");
        b.depends_on = vec!["a".to_string()];
        map.insert("b".to_string(), b);

        let res = DependencyGraph::build(&map);
        assert!(res.is_err(), "Cyclic dependency must be rejected");
    }

    #[test]
    fn test_unknown_dependency_detection() {
        let mut map = HashMap::new();

        let mut a = ProgramConfig::new("a", "echo a");
        a.depends_on = vec!["non_existent".to_string()];
        map.insert("a".to_string(), a);

        let res = DependencyGraph::build(&map);
        assert!(res.is_err(), "Unknown dependency must be rejected");
    }
}
