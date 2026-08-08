//! Dependency graph: DAG build + topological sort.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::config::ServiceConfig;
use crate::error::{Error, Result};

/// Topologically sorted service names. Detects cycles.
///
/// Among services that are currently ready (`indegree == 0`), always picks the
/// global minimum of `(order_priority, name)`. Lower `orderPriority` starts
/// earlier; equal priority falls back to alphabetical name.
pub fn topological_sort(services: &[ServiceConfig]) -> Result<Vec<String>> {
    let names: HashSet<&str> = services.iter().map(|s| s.name.as_str()).collect();
    let prio: HashMap<&str, u64> = services
        .iter()
        .map(|s| (s.name.as_str(), s.order_priority))
        .collect();
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for s in services {
        indegree.entry(s.name.as_str()).or_insert(0);
        adj.entry(s.name.as_str()).or_default();
    }

    for s in services {
        for dep in &s.depends_on {
            if !names.contains(dep.as_str()) {
                return Err(Error::Config(format!(
                    "service '{}': unknown dependency '{}'",
                    s.name, dep
                )));
            }
            // edge: dep -> s (dep must come first)
            adj.entry(dep.as_str()).or_default().push(s.name.as_str());
            let d = indegree.entry(s.name.as_str()).or_insert(0);
            *d += 1;
        }
    }

    let mut ready: BTreeSet<(u64, &str)> = indegree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&n, _)| (*prio.get(n).unwrap_or(&100), n))
        .collect();

    let mut order = Vec::with_capacity(services.len());
    while let Some((_, n)) = ready.pop_first() {
        order.push(n.to_string());
        if let Some(children) = adj.get(n) {
            for &c in children {
                let Some(d) = indegree.get_mut(c) else {
                    continue;
                };
                *d -= 1;
                if *d == 0 {
                    ready.insert((*prio.get(c).unwrap_or(&100), c));
                }
            }
        }
    }

    if order.len() != services.len() {
        let leftover: Vec<_> = indegree
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(&n, _)| n)
            .collect();
        debug_assert!(
            !leftover.is_empty(),
            "cycle detection: order incomplete but no positive indegree"
        );
        let involved = leftover.first().copied().unwrap_or("unknown");
        return Err(Error::Cycle(involved.to_string()));
    }

    Ok(order)
}

/// Split services into foreground (sequential) and background (parallel) lists,
/// both in topological order.
pub fn partition_boot(services: &[ServiceConfig]) -> Result<(Vec<String>, Vec<String>)> {
    let order = topological_sort(services)?;
    let by_name: HashMap<&str, &ServiceConfig> =
        services.iter().map(|s| (s.name.as_str(), s)).collect();

    let mut foreground = Vec::new();
    let mut background = Vec::new();
    for name in order {
        let Some(svc) = by_name.get(name.as_str()).copied() else {
            continue;
        };
        if !svc.enabled {
            continue;
        }
        if svc.background {
            background.push(name);
        } else {
            foreground.push(name);
        }
    }
    Ok((foreground, background))
}

/// Reverse topological order for shutdown.
pub fn shutdown_order(services: &[ServiceConfig]) -> Result<Vec<String>> {
    let mut order = topological_sort(services)?;
    order.reverse();
    Ok(order)
}
