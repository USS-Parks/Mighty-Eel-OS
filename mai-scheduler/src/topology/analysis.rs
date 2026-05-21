//! Precomputed topology analysis structures.
//!
//! Built once from the static topology graph. Provides:
//! - Best GPU pairs for 2-way tensor parallelism (sorted by link quality)
//! - Best GPU quads for 4-way tensor parallelism
//! - NVLink cliques (groups where ALL pairwise links are NVLink)
//! - Path cost matrix (Floyd-Warshall shortest paths)
//! - CPU affinity groups (GPUs sharing the same NUMA node)

use std::collections::HashMap;

use super::TopologyConfig;
use super::graph::GpuGraph;
use crate::types::GpuId;

/// Precomputed topology analysis results.
#[derive(Debug, Clone)]
pub struct PrecomputedTopology {
    /// Best GPU pairs for 2-way tensor parallelism, sorted by cost (lowest first).
    /// Each entry: (gpu_a, gpu_b, pair_cost).
    pub best_pairs: Vec<(GpuId, GpuId, f64)>,

    /// Best GPU quads for 4-way tensor parallelism, sorted by aggregate cost.
    /// Each entry: ([gpu_a, gpu_b, gpu_c, gpu_d], aggregate_cost).
    pub best_quads: Vec<([GpuId; 4], f64)>,

    /// NVLink cliques: groups of GPUs where ALL pairwise connections are NVLink.
    pub nvlink_cliques: Vec<Vec<GpuId>>,

    /// CPU affinity groups: GPUs sharing the same NUMA node.
    pub cpu_affinity_groups: Vec<Vec<GpuId>>,

    /// Floyd-Warshall shortest path cost matrix.
    /// Indexed by (from_gpu_ordinal, to_gpu_ordinal).
    path_costs: HashMap<(u32, u32), f64>,

    /// Ordered GPU IDs for matrix indexing.
    _gpu_ids: Vec<GpuId>,
}

impl PrecomputedTopology {
    /// Compute all analysis structures from the topology graph.
    pub fn compute(graph: &GpuGraph, _config: &TopologyConfig) -> Self {
        let gpu_ids = graph.gpu_ids();
        let path_costs = floyd_warshall(graph, &gpu_ids);
        let best_pairs = compute_best_pairs(graph, &gpu_ids);
        let best_quads = compute_best_quads(graph, &gpu_ids, &path_costs);
        let nvlink_cliques = detect_nvlink_cliques(graph, &gpu_ids);
        let cpu_affinity_groups = compute_cpu_affinity_groups(graph, &gpu_ids);

        Self {
            best_pairs,
            best_quads,
            nvlink_cliques,
            cpu_affinity_groups,
            path_costs,
            _gpu_ids: gpu_ids,
        }
    }

    /// Get the shortest path cost between two GPUs.
    pub fn path_cost(&self, a: GpuId, b: GpuId) -> f64 {
        if a == b {
            return 0.0;
        }
        self.path_costs
            .get(&(a.0, b.0))
            .copied()
            .unwrap_or(f64::INFINITY)
    }

    /// Compute the worst-case pair cost among a set of GPUs.
    /// This is the topology penalty: the maximum shortest-path cost
    /// between any two GPUs in the assignment.
    pub fn worst_pair_cost(&self, gpu_ids: &[GpuId]) -> f64 {
        let mut worst = 0.0_f64;
        for i in 0..gpu_ids.len() {
            for j in (i + 1)..gpu_ids.len() {
                let cost = self.path_cost(gpu_ids[i], gpu_ids[j]);
                if cost > worst {
                    worst = cost;
                }
            }
        }
        worst
    }
}

// ---------------------------------------------------------------------------
// Floyd-Warshall
// ---------------------------------------------------------------------------

/// Compute all-pairs shortest path costs using Floyd-Warshall.
/// Cluster sizes are small (<16 GPUs), so O(n^3) is fine.
fn floyd_warshall(graph: &GpuGraph, gpu_ids: &[GpuId]) -> HashMap<(u32, u32), f64> {
    let n = gpu_ids.len();
    // Initialize distance matrix
    let mut dist: Vec<Vec<f64>> = vec![vec![f64::INFINITY; n]; n];

    // Map GpuId -> index
    let id_to_idx: HashMap<GpuId, usize> =
        gpu_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();

    // Diagonal = 0
    for (i, row) in dist.iter_mut().enumerate().take(n) {
        row[i] = 0.0;
    }

    // Direct edges
    for (&(from, to), link) in graph.edges() {
        if let (Some(&i), Some(&j)) = (id_to_idx.get(&from), id_to_idx.get(&to)) {
            dist[i][j] = link.cost;
        }
    }

    // Floyd-Warshall relaxation
    for k in 0..n {
        for i in 0..n {
            for j in 0..n {
                let through_k = dist[i][k] + dist[k][j];
                if through_k < dist[i][j] {
                    dist[i][j] = through_k;
                }
            }
        }
    }

    // Convert back to HashMap keyed by GpuId ordinals
    let mut result = HashMap::new();
    for i in 0..n {
        for j in 0..n {
            if i != j {
                result.insert((gpu_ids[i].0, gpu_ids[j].0), dist[i][j]);
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Best pairs
// ---------------------------------------------------------------------------

/// Find all GPU pairs sorted by direct link cost (lowest first).
fn compute_best_pairs(graph: &GpuGraph, gpu_ids: &[GpuId]) -> Vec<(GpuId, GpuId, f64)> {
    let mut pairs = Vec::new();

    for i in 0..gpu_ids.len() {
        for j in (i + 1)..gpu_ids.len() {
            let cost = graph.link_cost(gpu_ids[i], gpu_ids[j]);
            if cost.is_finite() {
                pairs.push((gpu_ids[i], gpu_ids[j], cost));
            }
        }
    }

    pairs.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
    pairs
}

// ---------------------------------------------------------------------------
// Best quads
// ---------------------------------------------------------------------------

/// Find all GPU quads sorted by aggregate cost (sum of all 6 pairwise costs).
fn compute_best_quads(
    _graph: &GpuGraph,
    gpu_ids: &[GpuId],
    path_costs: &HashMap<(u32, u32), f64>,
) -> Vec<([GpuId; 4], f64)> {
    if gpu_ids.len() < 4 {
        return Vec::new();
    }

    let mut quads = Vec::new();
    let n = gpu_ids.len();

    for a in 0..n {
        for b in (a + 1)..n {
            for c in (b + 1)..n {
                for d in (c + 1)..n {
                    let ids = [gpu_ids[a], gpu_ids[b], gpu_ids[c], gpu_ids[d]];
                    let mut total_cost = 0.0;
                    let mut all_finite = true;

                    // Sum all 6 pairwise shortest-path costs
                    for i in 0..4 {
                        for j in (i + 1)..4 {
                            let cost = if ids[i] == ids[j] {
                                0.0
                            } else {
                                path_costs
                                    .get(&(ids[i].0, ids[j].0))
                                    .copied()
                                    .unwrap_or(f64::INFINITY)
                            };
                            if cost.is_infinite() {
                                all_finite = false;
                                break;
                            }
                            total_cost += cost;
                        }
                        if !all_finite {
                            break;
                        }
                    }

                    if all_finite {
                        quads.push((ids, total_cost));
                    }
                }
            }
        }
    }

    quads.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    quads
}

// ---------------------------------------------------------------------------
// NVLink clique detection
// ---------------------------------------------------------------------------

/// Find NVLink cliques: groups of GPUs where ALL pairwise links are NVLink.
///
/// Uses a greedy approach: start with the largest potential clique and
/// verify all pairs. For small GPU counts (<16), this is fast enough.
fn detect_nvlink_cliques(graph: &GpuGraph, gpu_ids: &[GpuId]) -> Vec<Vec<GpuId>> {
    if gpu_ids.len() <= 1 {
        return Vec::new();
    }

    // Build adjacency for NVLink connections
    let n = gpu_ids.len();
    let mut nvlink_adj: Vec<Vec<bool>> = vec![vec![false; n]; n];
    for i in 0..n {
        for j in 0..n {
            if i != j && graph.is_nvlink(gpu_ids[i], gpu_ids[j]) {
                nvlink_adj[i][j] = true;
            }
        }
    }

    // Find maximal cliques using Bron-Kerbosch (simplified for small n)
    let mut r: Vec<usize> = Vec::new();
    let p: Vec<usize> = (0..n).collect();
    let x: Vec<usize> = Vec::new();

    let mut all_cliques: Vec<Vec<usize>> = Vec::new();
    bron_kerbosch(&nvlink_adj, &mut r, &p, &x, &mut all_cliques);

    // Filter: only cliques of size >= 2 (a single GPU is not interesting)
    let mut result: Vec<Vec<GpuId>> = all_cliques
        .into_iter()
        .filter(|c| c.len() >= 2)
        .map(|c| {
            let mut ids: Vec<GpuId> = c.iter().map(|&i| gpu_ids[i]).collect();
            ids.sort_by_key(|id| id.0);
            ids
        })
        .collect();

    // Sort cliques by size (largest first), then by first GPU ID
    result.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].0.cmp(&b[0].0)));

    result
}

/// Bron-Kerbosch algorithm for maximal clique enumeration.
fn bron_kerbosch(
    adj: &[Vec<bool>],
    r: &mut Vec<usize>,
    p: &[usize],
    x: &[usize],
    cliques: &mut Vec<Vec<usize>>,
) {
    if p.is_empty() && x.is_empty() {
        if r.len() >= 2 {
            cliques.push(r.clone());
        }
        return;
    }

    // Choose pivot (vertex in P union X with most connections to P)
    let pivot = p
        .iter()
        .chain(x.iter())
        .max_by_key(|&&v| p.iter().filter(|&&u| adj[v][u]).count())
        .copied();

    let candidates: Vec<usize> = if let Some(pv) = pivot {
        p.iter().filter(|&&v| !adj[pv][v]).copied().collect()
    } else {
        p.to_vec()
    };

    let mut p_remaining = p.to_vec();
    let mut x_current = x.to_vec();

    for v in candidates {
        r.push(v);

        let p_new: Vec<usize> = p_remaining
            .iter()
            .filter(|&&u| adj[v][u])
            .copied()
            .collect();
        let x_new: Vec<usize> = x_current.iter().filter(|&&u| adj[v][u]).copied().collect();

        bron_kerbosch(adj, r, &p_new, &x_new, cliques);

        r.pop();
        p_remaining.retain(|&u| u != v);
        x_current.push(v);
    }
}

// ---------------------------------------------------------------------------
// CPU affinity groups
// ---------------------------------------------------------------------------

/// Group GPUs by NUMA node (CPU affinity).
fn compute_cpu_affinity_groups(graph: &GpuGraph, gpu_ids: &[GpuId]) -> Vec<Vec<GpuId>> {
    let mut groups: HashMap<u32, Vec<GpuId>> = HashMap::new();

    for &id in gpu_ids {
        if let Some(node) = graph.node(id) {
            if let Some(numa) = node.numa_node {
                groups.entry(numa).or_default().push(id);
            }
        }
    }

    let mut result: Vec<Vec<GpuId>> = groups.into_values().collect();
    // Sort groups by first GPU ID for deterministic output
    for group in &mut result {
        group.sort_by_key(|id| id.0);
    }
    result.sort_by(|a, b| a[0].0.cmp(&b[0].0));
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::LinkWeightConfig;
    use crate::topology::collector::{LinkType, ParsedGpu, ParsedLink, ParsedTopology};

    fn make_config() -> TopologyConfig {
        TopologyConfig::default()
    }

    fn make_two_gpu_nvlink_graph() -> GpuGraph {
        let parsed = ParsedTopology {
            gpus: vec![
                ParsedGpu {
                    gpu_id: GpuId(0),
                    name: "GPU0".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(1),
                    name: "GPU1".into(),
                    cpu_affinity: Some(0),
                },
            ],
            links: vec![
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(1),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(0),
                    link_type: LinkType::NV4,
                },
            ],
            cpu_affinity: [(GpuId(0), 0), (GpuId(1), 0)].into_iter().collect(),
        };
        GpuGraph::from_parsed(parsed, &LinkWeightConfig::default(), 1.0, 1.0)
    }

    fn make_four_gpu_mixed_graph() -> GpuGraph {
        let parsed = ParsedTopology {
            gpus: vec![
                ParsedGpu {
                    gpu_id: GpuId(0),
                    name: "GPU0".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(1),
                    name: "GPU1".into(),
                    cpu_affinity: Some(0),
                },
                ParsedGpu {
                    gpu_id: GpuId(2),
                    name: "GPU2".into(),
                    cpu_affinity: Some(32),
                },
                ParsedGpu {
                    gpu_id: GpuId(3),
                    name: "GPU3".into(),
                    cpu_affinity: Some(32),
                },
            ],
            links: vec![
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(1),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(0),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(2),
                    link_type: LinkType::PHB,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(0),
                    link_type: LinkType::PHB,
                },
                ParsedLink {
                    from: GpuId(0),
                    to: GpuId(3),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(3),
                    to: GpuId(0),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(2),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(1),
                    link_type: LinkType::SYS,
                },
                ParsedLink {
                    from: GpuId(1),
                    to: GpuId(3),
                    link_type: LinkType::PHB,
                },
                ParsedLink {
                    from: GpuId(3),
                    to: GpuId(1),
                    link_type: LinkType::PHB,
                },
                ParsedLink {
                    from: GpuId(2),
                    to: GpuId(3),
                    link_type: LinkType::NV4,
                },
                ParsedLink {
                    from: GpuId(3),
                    to: GpuId(2),
                    link_type: LinkType::NV4,
                },
            ],
            cpu_affinity: [(GpuId(0), 0), (GpuId(1), 0), (GpuId(2), 32), (GpuId(3), 32)]
                .into_iter()
                .collect(),
        };
        GpuGraph::from_parsed(parsed, &LinkWeightConfig::default(), 1.0, 1.0)
    }

    #[test]
    fn test_floyd_warshall_two_gpu() {
        let graph = make_two_gpu_nvlink_graph();
        let gpu_ids = graph.gpu_ids();
        let costs = floyd_warshall(&graph, &gpu_ids);

        let cost_0_1 = costs.get(&(0, 1)).unwrap();
        let cost_1_0 = costs.get(&(1, 0)).unwrap();
        // Symmetric NV4 link
        assert!((cost_0_1 - cost_1_0).abs() < f64::EPSILON);
        assert!(cost_0_1.is_finite());
    }

    #[test]
    fn test_floyd_warshall_four_gpu_shortest_path() {
        let graph = make_four_gpu_mixed_graph();
        let gpu_ids = graph.gpu_ids();
        let costs = floyd_warshall(&graph, &gpu_ids);

        // Direct NV4 link (0->1) should be cheaper than indirect paths
        let _direct_01 = costs.get(&(0, 1)).unwrap();
        // Direct SYS link (0->3) vs path through 0->1->3(PHB) or 0->2(PHB)->3(NV4)
        let cost_03 = costs.get(&(0, 3)).unwrap();

        // The shortest path 0->3 should be <= direct SYS cost
        // because 0->2(PHB) + 2->3(NV4) might be cheaper
        let direct_sys_cost = graph.link_cost(GpuId(0), GpuId(3));
        assert!(*cost_03 <= direct_sys_cost + f64::EPSILON);
    }

    #[test]
    fn test_best_pairs_nvlink_first() {
        let graph = make_four_gpu_mixed_graph();
        let gpu_ids = graph.gpu_ids();
        let pairs = compute_best_pairs(&graph, &gpu_ids);

        // NV4 pairs (0,1) and (2,3) should be first
        assert!(pairs.len() >= 2);
        let first_pair = &pairs[0];
        let second_pair = &pairs[1];

        // Both best pairs should be NV4
        assert!(
            graph.is_nvlink(first_pair.0, first_pair.1),
            "first pair should be NVLink"
        );
        assert!(
            graph.is_nvlink(second_pair.0, second_pair.1),
            "second pair should be NVLink"
        );

        // Non-NVLink pairs should come after
        if pairs.len() > 2 {
            let last_pair = pairs.last().unwrap();
            assert!(last_pair.2 >= first_pair.2);
        }
    }

    #[test]
    fn test_nvlink_cliques_two_gpu() {
        let graph = make_two_gpu_nvlink_graph();
        let gpu_ids = graph.gpu_ids();
        let cliques = detect_nvlink_cliques(&graph, &gpu_ids);

        assert_eq!(cliques.len(), 1);
        assert_eq!(cliques[0], vec![GpuId(0), GpuId(1)]);
    }

    #[test]
    fn test_nvlink_cliques_four_gpu_mixed() {
        let graph = make_four_gpu_mixed_graph();
        let gpu_ids = graph.gpu_ids();
        let cliques = detect_nvlink_cliques(&graph, &gpu_ids);

        // Should find two NVLink cliques: {0,1} and {2,3}
        assert_eq!(cliques.len(), 2);
        assert!(cliques.contains(&vec![GpuId(0), GpuId(1)]));
        assert!(cliques.contains(&vec![GpuId(2), GpuId(3)]));
    }

    #[test]
    fn test_cpu_affinity_groups() {
        let graph = make_four_gpu_mixed_graph();
        let gpu_ids = graph.gpu_ids();
        let groups = compute_cpu_affinity_groups(&graph, &gpu_ids);

        assert_eq!(groups.len(), 2);
        // Socket 0: GPU0, GPU1
        assert!(groups.iter().any(|g| *g == vec![GpuId(0), GpuId(1)]));
        // Socket 1: GPU2, GPU3
        assert!(groups.iter().any(|g| *g == vec![GpuId(2), GpuId(3)]));
    }

    #[test]
    fn test_worst_pair_cost() {
        let graph = make_four_gpu_mixed_graph();
        let config = make_config();
        let analysis = PrecomputedTopology::compute(&graph, &config);

        // Single GPU: 0.0
        assert_eq!(analysis.worst_pair_cost(&[GpuId(0)]), 0.0);

        // NVLink pair: low cost
        let nv_cost = analysis.worst_pair_cost(&[GpuId(0), GpuId(1)]);

        // Mixed pair: higher cost
        let mixed_cost = analysis.worst_pair_cost(&[GpuId(0), GpuId(3)]);

        assert!(nv_cost < mixed_cost);
    }

    #[test]
    fn test_path_cost_self_zero() {
        let graph = make_two_gpu_nvlink_graph();
        let config = make_config();
        let analysis = PrecomputedTopology::compute(&graph, &config);

        assert_eq!(analysis.path_cost(GpuId(0), GpuId(0)), 0.0);
    }

    #[test]
    fn test_best_quads_with_four_gpus() {
        let graph = make_four_gpu_mixed_graph();
        let gpu_ids = graph.gpu_ids();
        let costs = floyd_warshall(&graph, &gpu_ids);
        let quads = compute_best_quads(&graph, &gpu_ids, &costs);

        // With 4 GPUs, there's exactly 1 quad
        assert_eq!(quads.len(), 1);
        let (ids, cost) = &quads[0];
        assert_eq!(ids.len(), 4);
        assert!(cost.is_finite());
    }

    #[test]
    fn test_best_quads_insufficient_gpus() {
        let graph = make_two_gpu_nvlink_graph();
        let gpu_ids = graph.gpu_ids();
        let costs = floyd_warshall(&graph, &gpu_ids);
        let quads = compute_best_quads(&graph, &gpu_ids, &costs);

        assert!(quads.is_empty());
    }

    #[test]
    fn test_topology_penalty_nvlink_vs_pcie() {
        let graph = make_four_gpu_mixed_graph();
        let config = make_config();
        let topo = super::super::GpuTopology::from_graph(graph, config);

        let nvlink_penalty = topo.topology_penalty(&[GpuId(0), GpuId(1)]);
        let pcie_penalty = topo.topology_penalty(&[GpuId(0), GpuId(3)]);

        assert!(
            nvlink_penalty < pcie_penalty,
            "NVLink penalty ({nvlink_penalty}) should be lower than PCIe penalty ({pcie_penalty})"
        );
    }
}
