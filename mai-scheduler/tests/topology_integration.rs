//! Integration tests for GPU topology discovery pipeline.
//!
//! These tests read nvidia-smi fixture files from disk and run the full
//! parse -> graph -> analysis -> topology_penalty pipeline, verifying
//! structural properties at each stage.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use mai_scheduler::topology::collector::{LinkType, parse_topo_matrix};
use mai_scheduler::topology::graph::GpuGraph;
use mai_scheduler::topology::{GpuTopology, TopologyConfig};
use mai_scheduler::types::GpuId;

/// Resolve path to a fixture file relative to the crate root.
fn fixture_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push(name);
    path
}

/// Read a fixture file and return its contents.
fn read_fixture(name: &str) -> String {
    let path = fixture_path(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {}", path.display(), e))
}

// ---------------------------------------------------------------------------
// Single GPU fixture
// ---------------------------------------------------------------------------

#[test]
fn test_single_gpu_fixture_parses() {
    let raw = read_fixture("topo_single_gpu.txt");
    let parsed = parse_topo_matrix(&raw).expect("single GPU fixture should parse");
    assert_eq!(parsed.gpus.len(), 1, "expected 1 GPU");
    assert_eq!(parsed.gpus[0].gpu_id, GpuId(0));
    // Single GPU has no inter-GPU links
    assert!(parsed.links.is_empty(), "single GPU should have no links");
}

#[test]
fn test_single_gpu_topology_penalty_zero() {
    let raw = read_fixture("topo_single_gpu.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = GpuTopology::from_graph(graph, config);

    assert_eq!(topo.gpu_count(), 1);
    assert_eq!(topo.topology_penalty(&[GpuId(0)]), 0.0);
    assert_eq!(topo.topology_penalty(&[]), 0.0);
}

// ---------------------------------------------------------------------------
// 2-GPU NVLink fixture
// ---------------------------------------------------------------------------

#[test]
fn test_2gpu_nvlink_fixture_parses() {
    let raw = read_fixture("topo_2gpu_nvlink.txt");
    let parsed = parse_topo_matrix(&raw).expect("2-GPU NVLink fixture should parse");
    assert_eq!(parsed.gpus.len(), 2);
    // 2 directed edges: 0->1 and 1->0
    assert_eq!(parsed.links.len(), 2, "expected 2 directed NVLink edges");
    for link in &parsed.links {
        assert!(
            link.link_type.is_nvlink(),
            "all links in 2-GPU fixture should be NVLink, got {:?}",
            link.link_type
        );
    }
}

#[test]
fn test_2gpu_nvlink_graph_structure() {
    let raw = read_fixture("topo_2gpu_nvlink.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );

    assert_eq!(graph.node_count(), 2);
    assert_eq!(graph.edge_count(), 2);
    assert!(graph.is_nvlink(GpuId(0), GpuId(1)));
    assert!(graph.is_nvlink(GpuId(1), GpuId(0)));

    // NVLink cost should be low (latency=1, bw=900)
    let cost = graph.link_cost(GpuId(0), GpuId(1));
    assert!(cost < 2.0, "NV4 cost should be near 1.0, got {cost}");
}

#[test]
fn test_2gpu_nvlink_topology_analysis() {
    let raw = read_fixture("topo_2gpu_nvlink.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = GpuTopology::from_graph(graph, config);

    // Best pair should be (0, 1)
    let pairs = topo.best_pairs();
    assert!(!pairs.is_empty(), "should have at least one best pair");
    let (a, b, _cost) = &pairs[0];
    assert!(
        (*a == GpuId(0) && *b == GpuId(1)) || (*a == GpuId(1) && *b == GpuId(0)),
        "best pair should be GPUs 0 and 1"
    );

    // NVLink cliques should contain {0, 1}
    let cliques = topo.nvlink_cliques();
    assert!(
        !cliques.is_empty(),
        "should find at least one NVLink clique"
    );
    let clique = &cliques[0];
    assert!(clique.contains(&GpuId(0)) && clique.contains(&GpuId(1)));

    // Penalty for the NVLink pair should be low
    let penalty = topo.topology_penalty(&[GpuId(0), GpuId(1)]);
    assert!(
        penalty < 2.0,
        "NVLink pair penalty should be low, got {penalty}"
    );
}

// ---------------------------------------------------------------------------
// 4-GPU mixed topology fixture
// ---------------------------------------------------------------------------

#[test]
fn test_4gpu_mixed_fixture_parses() {
    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed = parse_topo_matrix(&raw).expect("4-GPU mixed fixture should parse");
    assert_eq!(parsed.gpus.len(), 4);
    // 4 GPUs, each has 3 links to others = 12 directed edges
    assert_eq!(
        parsed.links.len(),
        12,
        "expected 12 directed edges for 4 GPUs"
    );
}

#[test]
fn test_4gpu_mixed_cost_ordering() {
    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );

    // From fixture: GPU0-GPU1 = NV4, GPU0-GPU2 = PHB, GPU0-GPU3 = SYS
    let nv4_cost = graph.link_cost(GpuId(0), GpuId(1));
    let phb_cost = graph.link_cost(GpuId(0), GpuId(2));
    let sys_cost = graph.link_cost(GpuId(0), GpuId(3));

    assert!(
        nv4_cost < phb_cost,
        "NV4 ({nv4_cost}) should cost less than PHB ({phb_cost})"
    );
    assert!(
        phb_cost < sys_cost,
        "PHB ({phb_cost}) should cost less than SYS ({sys_cost})"
    );
}

#[test]
fn test_4gpu_mixed_topology_penalty_ordering() {
    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = GpuTopology::from_graph(graph, config);

    // NVLink pair (0,1) should have lower penalty than cross-socket (0,3)
    let nv_penalty = topo.topology_penalty(&[GpuId(0), GpuId(1)]);
    let cross_penalty = topo.topology_penalty(&[GpuId(0), GpuId(3)]);
    assert!(
        nv_penalty < cross_penalty,
        "NVLink pair penalty ({nv_penalty}) should be less than cross-socket ({cross_penalty})"
    );

    // 4-GPU penalty should be >= 2-GPU penalty (more pairs to consider)
    let quad_penalty = topo.topology_penalty(&[GpuId(0), GpuId(1), GpuId(2), GpuId(3)]);
    assert!(
        quad_penalty >= nv_penalty,
        "4-GPU penalty ({quad_penalty}) should be >= 2-GPU NVLink penalty ({nv_penalty})"
    );
}

#[test]
fn test_4gpu_mixed_cpu_affinity() {
    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();

    // GPUs 0,1 have CPU affinity 0-31, GPUs 2,3 have 32-63
    let cpu0 = parsed.cpu_affinity.get(&GpuId(0)).copied();
    let cpu1 = parsed.cpu_affinity.get(&GpuId(1)).copied();
    let cpu2 = parsed.cpu_affinity.get(&GpuId(2)).copied();
    let cpu3 = parsed.cpu_affinity.get(&GpuId(3)).copied();

    assert_eq!(cpu0, cpu1, "GPUs 0 and 1 should share CPU affinity");
    assert_eq!(cpu2, cpu3, "GPUs 2 and 3 should share CPU affinity");
    assert_ne!(
        cpu0, cpu2,
        "GPU 0 and GPU 2 should have different CPU affinity"
    );
}

#[test]
fn test_4gpu_mixed_nvlink_cliques() {
    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = GpuTopology::from_graph(graph, config);

    let cliques = topo.nvlink_cliques();
    // Should find two NVLink pairs: {0,1} and {2,3}
    assert_eq!(
        cliques.len(),
        2,
        "expected 2 NVLink cliques, got {}",
        cliques.len()
    );

    // Verify the cliques contain the right GPUs
    let has_01 = cliques
        .iter()
        .any(|c| c.contains(&GpuId(0)) && c.contains(&GpuId(1)));
    let has_23 = cliques
        .iter()
        .any(|c| c.contains(&GpuId(2)) && c.contains(&GpuId(3)));
    assert!(has_01, "should find NVLink clique containing GPUs 0 and 1");
    assert!(has_23, "should find NVLink clique containing GPUs 2 and 3");
}

// ---------------------------------------------------------------------------
// 8-GPU DGX-like fixture
// ---------------------------------------------------------------------------

#[test]
fn test_8gpu_dgx_fixture_parses() {
    let raw = read_fixture("topo_8gpu_dgx.txt");
    let parsed = parse_topo_matrix(&raw).expect("8-GPU DGX fixture should parse");
    assert_eq!(parsed.gpus.len(), 8);
    // 8 GPUs, each has 7 links = 56 directed edges
    assert_eq!(
        parsed.links.len(),
        56,
        "expected 56 directed edges for 8 GPUs"
    );
}

#[test]
fn test_8gpu_dgx_nvlink_dense() {
    let raw = read_fixture("topo_8gpu_dgx.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();

    // Count NVLink vs PHB edges
    let nvlink_count = parsed
        .links
        .iter()
        .filter(|l| l.link_type.is_nvlink())
        .count();
    let phb_count = parsed
        .links
        .iter()
        .filter(|l| l.link_type == LinkType::PHB)
        .count();

    // DGX fixture has a dense NVLink mesh within each socket
    assert!(
        nvlink_count > phb_count,
        "DGX should have more NVLink ({nvlink_count}) than PHB ({phb_count}) edges"
    );
}

#[test]
fn test_8gpu_dgx_topology_structure() {
    let raw = read_fixture("topo_8gpu_dgx.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = GpuTopology::from_graph(graph, config);

    assert_eq!(topo.gpu_count(), 8);

    // NVLink cliques should exist (DGX has dense NVLink within sockets)
    let cliques = topo.nvlink_cliques();
    assert!(!cliques.is_empty(), "DGX should have NVLink cliques");

    // The largest clique should have at least 4 GPUs (full intra-socket mesh)
    let max_clique_size = cliques.iter().map(|c| c.len()).max().unwrap_or(0);
    assert!(
        max_clique_size >= 4,
        "DGX should have NVLink cliques of size >= 4, max is {max_clique_size}"
    );

    // Best pairs should prefer intra-socket NVLink pairs
    let pairs = topo.best_pairs();
    assert!(!pairs.is_empty());
    let (a, b, cost) = &pairs[0];
    assert!(
        cost < &2.0,
        "Best pair ({a:?}, {b:?}) should have NVLink-level cost, got {cost}"
    );
}

#[test]
fn test_8gpu_dgx_cpu_affinity_groups() {
    let raw = read_fixture("topo_8gpu_dgx.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = GpuTopology::from_graph(graph, config);

    let groups = topo.cpu_affinity_groups();
    // DGX has 2 CPU sockets: GPUs 0-3 on socket 0, GPUs 4-7 on socket 1
    assert_eq!(groups.len(), 2, "DGX should have 2 CPU affinity groups");

    let total_gpus: usize = groups.iter().map(|g| g.len()).sum();
    assert_eq!(total_gpus, 8, "all 8 GPUs should be in affinity groups");
}

// ---------------------------------------------------------------------------
// End-to-end: config weight sensitivity
// ---------------------------------------------------------------------------

#[test]
fn test_config_weights_affect_edge_cost() {
    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed1 = parse_topo_matrix(&raw).unwrap();
    let parsed2 = parse_topo_matrix(&raw).unwrap();

    // Default weights
    let config1 = TopologyConfig::default();
    let graph1 = GpuGraph::from_parsed(
        &parsed1,
        &config1.link_weights,
        config1.latency_weight,
        config1.bw_weight,
    );
    let cost1 = graph1.link_cost(GpuId(0), GpuId(3)); // SYS link

    // Double the latency weight
    let mut config2 = TopologyConfig::default();
    config2.latency_weight = 2.0;
    let graph2 = GpuGraph::from_parsed(
        &parsed2,
        &config2.link_weights,
        config2.latency_weight,
        config2.bw_weight,
    );
    let cost2 = graph2.link_cost(GpuId(0), GpuId(3)); // same SYS link

    assert!(
        cost2 > cost1,
        "Doubling latency_weight should increase SYS link cost: {cost1} vs {cost2}"
    );
}

#[test]
fn test_placement_engine_topology_integration() {
    use mai_scheduler::placement::PlacementEngine;

    let raw = read_fixture("topo_4gpu_mixed.txt");
    let parsed = parse_topo_matrix(&raw).unwrap();
    let config = TopologyConfig::default();
    let graph = GpuGraph::from_parsed(
        &parsed,
        &config.link_weights,
        config.latency_weight,
        config.bw_weight,
    );
    let topo = Arc::new(GpuTopology::from_graph(graph, config));

    let mut engine = PlacementEngine::new(64);
    engine.set_topology(Arc::clone(&topo));

    // NVLink pair should have lower penalty than cross-socket pair
    let nv_penalty = engine.topology_penalty(&[GpuId(0), GpuId(1)]);
    let sys_penalty = engine.topology_penalty(&[GpuId(0), GpuId(3)]);
    assert!(
        nv_penalty < sys_penalty,
        "PlacementEngine: NV penalty ({nv_penalty}) < SYS penalty ({sys_penalty})"
    );
}
