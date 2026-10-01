use crate::models::{DEFAULT_SPAWNS, OptimizerConfig, ResourceNode, SpawnLocation};
use rayon::prelude::*;
use std::collections::HashMap;

pub const MIN_X: f64 = -320000.0;
pub const MAX_X: f64 = 420000.0;
pub const MIN_Y: f64 = -370000.0;
pub const MAX_Y: f64 = 370000.0;

#[derive(Debug, Clone, serde::Serialize)]
pub struct OptimizationResult {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub score: f64,
    pub closest_spawn: SpawnLocation,
    pub spawn_distance: f64,
    /// Counts of accessible (non-obstructed) local nodes, keyed by "Purity Type"
    pub local_nodes: HashMap<String, u32>,
    /// Counts of obstructed local nodes (require Nobelisk) keyed by "Purity Type"
    pub obstructed_nodes: HashMap<String, u32>,
    /// Decay-weighted yield per resource type (the values the utility function actually used)
    pub resource_yields: HashMap<String, f64>,
    /// Terrain Ruggedness Index: mean absolute Z-difference to spatial neighbours (metres)
    pub terrain_ruggedness: f64,
    /// Shannon entropy diversity of resource yields (higher = more balanced)
    pub diversity_score: f64,
}

#[derive(Debug, Clone, Copy)]
struct OptNode {
    x: f64,
    y: f64,
    z: f64,
    res_idx: usize,
    multiplier: f64,
    obstructed: bool,
}

struct SpatialGrid {
    bucket_size: f64,
    cols: usize,
    rows: usize,
    min_x: f64,
    min_y: f64,
    buckets: Vec<Vec<usize>>, // Indices of nodes in OptNode list
}

impl SpatialGrid {
    fn new(nodes: &[OptNode], bucket_size: f64) -> Self {
        let min_x = MIN_X;
        let min_y = MIN_Y;
        let max_x = MAX_X;
        let max_y = MAX_Y;

        let cols = (((max_x - min_x) / bucket_size).ceil() as usize).max(1);
        let rows = (((max_y - min_y) / bucket_size).ceil() as usize).max(1);

        let mut buckets = vec![Vec::new(); cols * rows];

        for (idx, node) in nodes.iter().enumerate() {
            let col =
                (((node.x - min_x) / bucket_size) as isize).clamp(0, cols as isize - 1) as usize;
            let row =
                (((node.y - min_y) / bucket_size) as isize).clamp(0, rows as isize - 1) as usize;
            buckets[row * cols + col].push(idx);
        }

        Self {
            bucket_size,
            cols,
            rows,
            min_x,
            min_y,
            buckets,
        }
    }
}

/// Helper that calculates the minimum distance in metres from (x, y) to any
/// static water body. The old "coast edge" approximations (e.g. x < -250000 → dist=0)
/// have been REMOVED: those regions are the map's impassable mountain walls, not
/// accessible ocean. Treating them as free water caused the optimizer to inflate
/// scores at the western/northern map boundary and produce border-edge results.
fn distance_to_nearest_water(x: f64, y: f64, waterwell_nodes: &[(f64, f64)]) -> f64 {
    let mut min_dist_cm = f64::MAX;

    // Major static bodies of water in Satisfactory (centres, half-widths, in cm)
    let water_bodies = [
        (140000.0, 230000.0, 80000.0, 60000.0), // Great Blue Crater Lake (South-East)
        (-70000.0, -145000.0, 60000.0, 70000.0), // Crater Lakes / Coal Lakes (Northern Forest)
        (-60000.0, 65000.0, 80000.0, 70000.0),  // Red Jungle Lakes
        (45000.0, -20000.0, 30000.0, 30000.0),  // Lake Forest Lake
        (350000.0, 225000.0, 140000.0, 150000.0), // Eastern Swamp Lakes
        (310000.0, -185000.0, 25000.0, 20000.0), // Southern Dune Desert pond cluster
        (290000.0, -230000.0, 20000.0, 15000.0), // Far-south Dune Desert pond
        (355000.0, -80000.0, 30000.0, 25000.0), // Central-east Dune Desert oasis
    ];

    for &(cx, cy, w, h) in &water_bodies {
        let dx = (x - cx).abs() - w / 2.0;
        let dy = (y - cy).abs() - h / 2.0;
        let dist = if dx > 0.0 && dy > 0.0 {
            (dx * dx + dy * dy).sqrt()
        } else if dx > 0.0 {
            dx
        } else if dy > 0.0 {
            dy
        } else {
            0.0 // Inside the water body bounds
        };
        if dist < min_dist_cm {
            min_dist_cm = dist;
        }
    }

    let mut min_dist_m = min_dist_cm / 100.0;

    // Add dynamic waterwell checks
    for &(node_x, node_y) in waterwell_nodes {
        let dx = (x - node_x) / 100.0;
        let dy = (y - node_y) / 100.0;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < min_dist_m {
            min_dist_m = dist;
        }
    }

    min_dist_m
}

fn decay_weight(
    distance_m: f64,
    distance_sq_m: f64,
    sigma: f64,
    decay_func: crate::models::DistanceDecay,
) -> f64 {
    match decay_func {
        crate::models::DistanceDecay::Gaussian => {
            let two_sigma_sq = 2.0 * sigma * sigma;
            (-distance_sq_m / two_sigma_sq).exp()
        }
        crate::models::DistanceDecay::Exponential => (-distance_m / sigma).exp(),
        crate::models::DistanceDecay::PowerLaw => 1.0 / (distance_m / sigma + 1.0),
        crate::models::DistanceDecay::Linear => (1.0 - distance_m / sigma).max(0.0),
        crate::models::DistanceDecay::LogisticStep => {
            if distance_m <= sigma {
                1.0
            } else {
                0.05
            }
        }
    }
}

fn obstructed_node_contributes(obstructed: bool, game_phase: crate::models::GamePhase) -> bool {
    !obstructed
        || (game_phase != crate::models::GamePhase::Phase1
            && game_phase != crate::models::GamePhase::Phase2)
}

fn node_yield_contribution(
    node: &OptNode,
    decay: f64,
    game_phase: crate::models::GamePhase,
) -> f64 {
    if obstructed_node_contributes(node.obstructed, game_phase) {
        node.multiplier * decay
    } else {
        0.0
    }
}

fn virtual_water_yield(
    x: f64,
    y: f64,
    waterwell_nodes: &[(f64, f64)],
    config: &OptimizerConfig,
) -> f64 {
    let water_dist = distance_to_nearest_water(x, y, waterwell_nodes);
    decay_weight(
        water_dist,
        water_dist * water_dist,
        config.sigma,
        config.decay_func,
    )
}

/// Estimates the ground altitude Z at a coordinate (x, y) using KNN IDW (Inverse Distance Weighting)
fn estimate_altitude(
    x: f64,
    y: f64,
    opt_nodes: &[OptNode],
    spatial_grid: &SpatialGrid,
    sigma: f64,
) -> f64 {
    let radius = 3.5 * sigma;
    let radius_cm = radius * 100.0;
    let radius_cm_sq = radius_cm * radius_cm;

    let mut nearest = [(f64::MAX, 0.0); 6]; // (dist_sq, z)
    let mut count = 0;

    let min_qx = x - radius_cm;
    let max_qx = x + radius_cm;
    let min_qy = y - radius_cm;
    let max_qy = y + radius_cm;

    let col_start = (((min_qx - spatial_grid.min_x) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.cols as isize - 1) as usize;
    let col_end = (((max_qx - spatial_grid.min_x) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.cols as isize - 1) as usize;
    let row_start = (((min_qy - spatial_grid.min_y) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.rows as isize - 1) as usize;
    let row_end = (((max_qy - spatial_grid.min_y) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.rows as isize - 1) as usize;

    for r_idx in row_start..=row_end {
        for c_idx in col_start..=col_end {
            let bucket_idx = r_idx * spatial_grid.cols + c_idx;
            for &node_idx in &spatial_grid.buckets[bucket_idx] {
                let node = &opt_nodes[node_idx];
                let dx = x - node.x;
                let dy = y - node.y;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq <= radius_cm_sq {
                    if dist_sq < nearest[5].0 {
                        let mut insert_pos = 5;
                        while insert_pos > 0 && dist_sq < nearest[insert_pos - 1].0 {
                            insert_pos -= 1;
                        }
                        for idx in (insert_pos + 1..6).rev() {
                            nearest[idx] = nearest[idx - 1];
                        }
                        nearest[insert_pos] = (dist_sq, node.z);
                        if count < 6 {
                            count += 1;
                        }
                    }
                }
            }
        }
    }

    if count > 0 {
        let mut total_weight = 0.0;
        let mut weighted_z = 0.0;
        for i in 0..count {
            let dist_m = nearest[i].0.sqrt() / 100.0;
            let w = 1.0 / ((dist_m + 10.0) * (dist_m + 10.0));
            weighted_z += nearest[i].1 * w;
            total_weight += w;
        }
        weighted_z / total_weight
    } else {
        0.0
    }
}

const LAND_MASK_SECTORS: usize = 128;
const LAND_MASK_BUFFER_CM: f64 = 22_000.0; // ≈ 30 map pixels / 220 m.
const MAP_PIXEL_TO_CM: f64 = 1.0 / 0.0013653321;

const BORDER_MARGIN_CM: f64 = 30_000.0; // 300 m
const LAND_ACCEL_RES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvalQuality {
    /// Grid screening: land bitmap + distance field, 2D yields only (no KNN altitude).
    Coarse,
    /// Final scoring: exact border near edges, full 3D altitude + flatness.
    Fine,
}

#[derive(Debug, Clone)]
struct LandMask {
    points: Vec<(f64, f64)>,
}

/// Precomputed land membership + border distance for O(1) grid rejects.
#[derive(Debug, Clone)]
struct LandAccel {
    cols: usize,
    rows: usize,
    cell_w: f64,
    cell_h: f64,
    /// Negative => outside land. Otherwise distance to polygon edge in cm.
    border_dist_cm: Vec<f32>,
}

impl LandAccel {
    fn from_mask(mask: &LandMask) -> Self {
        let cols = LAND_ACCEL_RES;
        let rows = LAND_ACCEL_RES;
        let cell_w = (MAX_X - MIN_X) / cols as f64;
        let cell_h = (MAX_Y - MIN_Y) / rows as f64;
        let points = mask.points.clone();

        let border_dist_cm: Vec<f32> = (0..rows * cols)
            .into_par_iter()
            .map(|idx| {
                let c = idx % cols;
                let r = idx / cols;
                let x = MIN_X + (c as f64 + 0.5) * cell_w;
                let y = MIN_Y + (r as f64 + 0.5) * cell_h;
                if is_in_polygon(x, y, &points) {
                    dist_to_polygon_edge(x, y, &points) as f32
                } else {
                    -1.0
                }
            })
            .collect();

        Self {
            cols,
            rows,
            cell_w,
            cell_h,
            border_dist_cm,
        }
    }

    #[inline]
    fn sample(&self, x: f64, y: f64) -> Option<f64> {
        let c = ((x - MIN_X) / self.cell_w).floor() as isize;
        let r = ((y - MIN_Y) / self.cell_h).floor() as isize;
        if c < 0 || r < 0 || c >= self.cols as isize || r >= self.rows as isize {
            return None;
        }
        let d = self.border_dist_cm[r as usize * self.cols + c as usize];
        if d < 0.0 { None } else { Some(d as f64) }
    }
}

#[inline]
fn border_penalty_from_dist(border_dist_cm: f64) -> f64 {
    if border_dist_cm < BORDER_MARGIN_CM {
        (-4.0 * (1.0 - border_dist_cm / BORDER_MARGIN_CM)).exp()
    } else {
        1.0
    }
}

/// Returns border penalty, or None when the point is outside buildable land.
fn land_border_penalty(
    x: f64,
    y: f64,
    land_mask: &LandMask,
    land_accel: &LandAccel,
    quality: EvalQuality,
) -> Option<f64> {
    match land_accel.sample(x, y) {
        None => {
            if quality == EvalQuality::Coarse || !is_in_polygon(x, y, &land_mask.points) {
                None
            } else {
                let d = dist_to_polygon_edge(x, y, &land_mask.points);
                let p = border_penalty_from_dist(d);
                if p < 0.01 { None } else { Some(p) }
            }
        }
        Some(field_dist) if field_dist >= BORDER_MARGIN_CM => Some(1.0),
        Some(field_dist) => {
            let d = if quality == EvalQuality::Coarse {
                field_dist
            } else {
                dist_to_polygon_edge(x, y, &land_mask.points)
            };
            let p = border_penalty_from_dist(d);
            if p < 0.01 { None } else { Some(p) }
        }
    }
}

impl LandMask {
    fn from_nodes(nodes: &[OptNode]) -> Self {
        if nodes.len() < 3 {
            return Self {
                points: vec![
                    (MIN_X, MIN_Y),
                    (MAX_X, MIN_Y),
                    (MAX_X, MAX_Y),
                    (MIN_X, MAX_Y),
                ],
            };
        }

        let (sum_x, sum_y) = nodes
            .iter()
            .fold((0.0, 0.0), |(sx, sy), n| (sx + n.x, sy + n.y));
        let center_x = sum_x / nodes.len() as f64;
        let center_y = sum_y / nodes.len() as f64;

        let mut radii = vec![0.0_f64; LAND_MASK_SECTORS];
        for node in nodes {
            let dx = node.x - center_x;
            let dy = node.y - center_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist == 0.0 {
                continue;
            }
            let angle = dy.atan2(dx);
            let normalized = (angle + std::f64::consts::PI) / (2.0 * std::f64::consts::PI);
            let idx = ((normalized * LAND_MASK_SECTORS as f64).floor() as usize)
                .min(LAND_MASK_SECTORS - 1);
            radii[idx] = radii[idx].max(dist);
        }

        // Build an evenly sampled radial envelope instead of connecting the exact
        // farthest node positions. This keeps vertices uniformly distributed, fills
        // sparse sectors by interpolation, and produces a border with a consistent
        // outward clearance from the extremal nodes.
        let original_radii = radii.clone();
        for i in 0..LAND_MASK_SECTORS {
            if radii[i] > 0.0 {
                continue;
            }
            let mut prev = (i + LAND_MASK_SECTORS - 1) % LAND_MASK_SECTORS;
            while radii[prev] == 0.0 {
                prev = (prev + LAND_MASK_SECTORS - 1) % LAND_MASK_SECTORS;
            }
            let mut next = (i + 1) % LAND_MASK_SECTORS;
            while radii[next] == 0.0 {
                next = (next + 1) % LAND_MASK_SECTORS;
            }
            radii[i] = radii[prev].max(radii[next]);
        }

        for _ in 0..2 {
            let prev = radii.clone();
            for i in 0..LAND_MASK_SECTORS {
                let l = prev[(i + LAND_MASK_SECTORS - 1) % LAND_MASK_SECTORS];
                let r = prev[(i + 1) % LAND_MASK_SECTORS];
                radii[i] = original_radii[i].max((l + 2.0 * prev[i] + r) / 4.0);
            }
        }

        let buffer = LAND_MASK_BUFFER_CM.max(30.0 * MAP_PIXEL_TO_CM);
        let half_sector = std::f64::consts::PI / LAND_MASK_SECTORS as f64;
        let angular_margin = 1.0 / half_sector.cos();
        let points = radii
            .iter()
            .enumerate()
            .map(|(i, radius)| {
                let angle = -std::f64::consts::PI
                    + (i as f64 + 0.5) * 2.0 * std::f64::consts::PI / LAND_MASK_SECTORS as f64;
                let out_radius = radius * angular_margin + buffer;
                (
                    (center_x + angle.cos() * out_radius).clamp(MIN_X, MAX_X),
                    (center_y + angle.sin() * out_radius).clamp(MIN_Y, MAX_Y),
                )
            })
            .collect();

        Self { points }
    }
}

/// Ray-casting point-in-polygon test.
#[inline]
fn is_in_polygon(x: f64, y: f64, vs: &[(f64, f64)]) -> bool {
    let n = vs.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = vs[i];
        let (xj, yj) = vs[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Minimum distance from (x, y) to the nearest edge of the buildable-land polygon (in cm).
#[inline]
fn dist_to_polygon_edge(x: f64, y: f64, vs: &[(f64, f64)]) -> f64 {
    let n = vs.len();
    let mut min_dist = f64::MAX;
    let mut j = n - 1;
    for i in 0..n {
        let (ax, ay) = vs[j];
        let (bx, by) = vs[i];
        let dx = bx - ax;
        let dy = by - ay;
        let len_sq = dx * dx + dy * dy;
        let t = if len_sq > 0.0 {
            (((x - ax) * dx + (y - ay) * dy) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let proj_x = ax + t * dx;
        let proj_y = ay + t * dy;
        let ddx = x - proj_x;
        let ddy = y - proj_y;
        let dist = (ddx * ddx + ddy * ddy).sqrt();
        if dist < min_dist {
            min_dist = dist;
        }
        j = i;
    }
    min_dist
}

/// Calculates the dynamic multi-resource utility at (x, y).
fn calculate_utility(
    x: f64,
    y: f64,
    opt_nodes: &[OptNode],
    spatial_grid: &SpatialGrid,
    config: &OptimizerConfig,
    num_resources: usize,
    weights_arr: &[f64],
    epsilons_arr: &[f64],
    res_to_idx: &HashMap<String, usize>,
    waterwell_nodes: &[(f64, f64)],
    land_mask: &LandMask,
    land_accel: &LandAccel,
    quality: EvalQuality,
) -> f64 {
    let border_penalty = match land_border_penalty(x, y, land_mask, land_accel, quality) {
        Some(p) => p,
        None => return 0.0,
    };

    let radius = 3.5 * config.sigma;
    let radius_cm = radius * 100.0;
    let radius_m_sq = radius * radius;

    let min_qx = x - radius_cm;
    let max_qx = x + radius_cm;
    let min_qy = y - radius_cm;
    let max_qy = y + radius_cm;

    let col_start = (((min_qx - spatial_grid.min_x) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.cols as isize - 1) as usize;
    let col_end = (((max_qx - spatial_grid.min_x) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.cols as isize - 1) as usize;
    let row_start = (((min_qy - spatial_grid.min_y) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.rows as isize - 1) as usize;
    let row_end = (((max_qy - spatial_grid.min_y) / spatial_grid.bucket_size) as isize)
        .clamp(0, spatial_grid.rows as isize - 1) as usize;

    let use_altitude = quality == EvalQuality::Fine;
    let z = if use_altitude {
        estimate_altitude(x, y, opt_nodes, spatial_grid, config.sigma)
    } else {
        0.0
    };

    let mut yields = [0.0; 128];

    let build_radius = 1.5 * config.sigma;
    let build_radius_m_sq = build_radius * build_radius;

    let mut heights_count = 0;
    let mut heights_mean = 0.0;
    let mut heights_m2 = 0.0;

    for r_idx in row_start..=row_end {
        for c_idx in col_start..=col_end {
            let bucket_idx = r_idx * spatial_grid.cols + c_idx;
            for &node_idx in &spatial_grid.buckets[bucket_idx] {
                let node = &opt_nodes[node_idx];
                let dx = (x - node.x) / 100.0;
                let dy = (y - node.y) / 100.0;

                let d_sq = if use_altitude {
                    let dz = (z - node.z) / 100.0;
                    let vertical_multiplier = 4.0;
                    dx * dx + dy * dy + (dz * dz * vertical_multiplier * vertical_multiplier)
                } else {
                    dx * dx + dy * dy
                };

                if d_sq <= radius_m_sq {
                    let d = d_sq.sqrt();
                    let decay = decay_weight(d, d_sq, config.sigma, config.decay_func);
                    let contribution = node_yield_contribution(node, decay, config.game_phase);
                    yields[node.res_idx] += contribution;

                    if use_altitude && d_sq <= build_radius_m_sq {
                        heights_count += 1;
                        let delta = node.z - heights_mean;
                        heights_mean += delta / heights_count as f64;
                        let delta2 = node.z - heights_mean;
                        heights_m2 += delta * delta2;
                    }
                }
            }
        }
    }

    if let Some(&water_idx) = res_to_idx.get("water") {
        yields[water_idx] = virtual_water_yield(x, y, waterwell_nodes, config);
    }

    let mut flatness_mult = 1.0;
    if use_altitude && heights_count > 1 {
        let std_dev_cm = (heights_m2 / heights_count as f64).sqrt();
        let std_dev_m = std_dev_cm / 100.0;
        flatness_mult = (-std_dev_m / 30.0).exp();
    }

    for i in 0..num_resources {
        if yields[i] > 1.0 {
            yields[i] *= 1.0 + 0.1 * (yields[i] - 1.0);
        }
    }

    let mut score = match config.utility_func {
        crate::models::UtilityFunction::CobbDouglas => {
            let weight_sum: f64 = weights_arr[..num_resources]
                .iter()
                .filter(|&&w| w > 0.0)
                .sum();
            let norm = if weight_sum > 0.0 { weight_sum } else { 1.0 };
            let mut s = 1.0;
            for i in 0..num_resources {
                let weight = weights_arr[i];
                if weight > 0.0 {
                    let res_yield = yields[i];
                    let eps = epsilons_arr[i];
                    s *= (res_yield + eps).powf(weight / norm);
                }
            }
            s
        }
        crate::models::UtilityFunction::Leontief => {
            let mut s = f64::MAX;
            let mut has_pos = false;
            for i in 0..num_resources {
                let weight = weights_arr[i];
                if weight > 0.0 {
                    has_pos = true;
                    let res_yield = yields[i];
                    let eps = epsilons_arr[i];
                    let val = (res_yield + eps) / weight;
                    if val < s {
                        s = val;
                    }
                }
            }
            if has_pos { s } else { 0.0 }
        }
        crate::models::UtilityFunction::Linear => {
            let mut s = 0.0;
            for i in 0..num_resources {
                let weight = weights_arr[i];
                if weight > 0.0 {
                    s += yields[i] * weight;
                }
            }
            s
        }
    };

    for i in 0..num_resources {
        let weight = weights_arr[i];
        if weight < 0.0 {
            let res_yield = yields[i];
            let penalty_factor = 1.0 + res_yield * weight.abs();
            score /= penalty_factor;
        }
    }

    let spawn_tolerance_m: Option<f64> = if config.ignore_spawns {
        None
    } else {
        match config.game_phase {
            crate::models::GamePhase::Phase1 => Some(800.0),
            crate::models::GamePhase::Phase2 => Some(1500.0),
            crate::models::GamePhase::Phase3 => Some(3000.0),
            crate::models::GamePhase::Phase4 | crate::models::GamePhase::Phase5 => None,
        }
    };

    if let Some(tol) = spawn_tolerance_m {
        let mut min_spawn_dist_m = f64::MAX;
        for spawn in DEFAULT_SPAWNS {
            let dx = (x - spawn.x) / 100.0;
            let dy = (y - spawn.y) / 100.0;
            let d = (dx * dx + dy * dy).sqrt();
            let boundary_dist = (d - spawn.radius).max(0.0);
            if boundary_dist < min_spawn_dist_m {
                min_spawn_dist_m = boundary_dist;
            }
        }
        let spawn_penalty = (-min_spawn_dist_m / tol).exp();
        score *= spawn_penalty;
    }

    score * flatness_mult * border_penalty
}

/// Finite-difference gradient ascent, then assemble the result payload.
fn run_hill_climbing(
    start_x: f64,
    start_y: f64,
    opt_nodes: &[OptNode],
    spatial_grid: &SpatialGrid,
    config: &OptimizerConfig,
    num_resources: usize,
    weights_arr: &[f64],
    epsilons_arr: &[f64],
    res_to_idx: &HashMap<String, usize>,
    waterwell_nodes: &[(f64, f64)],
    land_mask: &LandMask,
    land_accel: &LandAccel,
) -> OptimizationResult {
    let util = |x: f64, y: f64| {
        calculate_utility(
            x,
            y,
            opt_nodes,
            spatial_grid,
            config,
            num_resources,
            weights_arr,
            epsilons_arr,
            res_to_idx,
            waterwell_nodes,
            land_mask,
            land_accel,
            EvalQuality::Fine,
        )
    };

    let mut curr_x = start_x;
    let mut curr_y = start_y;
    let mut step: f64 = 10000.0;
    let tolerance = 10.0;
    let mut max_score = util(curr_x, curr_y);

    while step > tolerance {
        let h = step.max(50.0);
        let gx = (util(curr_x + h, curr_y) - util(curr_x - h, curr_y)) / (2.0 * h);
        let gy = (util(curr_x, curr_y + h) - util(curr_x, curr_y - h)) / (2.0 * h);
        let gnorm = (gx * gx + gy * gy).sqrt();
        if gnorm < 1e-15 {
            step *= 0.5;
            continue;
        }

        let next_x = (curr_x + step * gx / gnorm).clamp(MIN_X, MAX_X);
        let next_y = (curr_y + step * gy / gnorm).clamp(MIN_Y, MAX_Y);
        let next_score = util(next_x, next_y);

        if next_score > max_score {
            max_score = next_score;
            curr_x = next_x;
            curr_y = next_y;
        } else {
            step *= 0.5;
        }
    }

    let final_z = estimate_altitude(curr_x, curr_y, opt_nodes, spatial_grid, config.sigma);

    let mut closest_spawn = DEFAULT_SPAWNS[0].clone();
    let mut min_dist = f64::MAX;

    for spawn in DEFAULT_SPAWNS {
        let dx = curr_x - spawn.x;
        let dy = curr_y - spawn.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < min_dist {
            min_dist = dist;
            closest_spawn = spawn.clone();
        }
    }

    let mut inv_res_map: HashMap<usize, String> = HashMap::new();
    for (k, v) in res_to_idx {
        inv_res_map.insert(*v, k.clone());
    }

    let search_radius_sq = (config.sigma * 100.0) * (config.sigma * 100.0);

    let mut local_nodes: HashMap<String, u32> = HashMap::new();
    let mut obstructed_nodes: HashMap<String, u32> = HashMap::new();
    let mut resource_yields: HashMap<String, f64> = HashMap::new();

    let mut tri_sum = 0.0;
    let mut tri_count = 0usize;
    let tri_radius_sq = (config.sigma * 0.5 * 100.0) * (config.sigma * 0.5 * 100.0);

    for node in opt_nodes {
        let dx = curr_x - node.x;
        let dy = curr_y - node.y;
        let dz = final_z - node.z;
        let d_sq_3d = dx * dx + dy * dy + (dz * dz * 16.0);
        let d_sq_2d = dx * dx + dy * dy;

        if d_sq_3d <= search_radius_sq {
            if let Some(name) = inv_res_map.get(&node.res_idx) {
                let purity_str = if node.multiplier > 1.5 {
                    "Pure"
                } else if node.multiplier < 0.8 {
                    "Impure"
                } else {
                    "Normal"
                };
                let display_name = format!("{} {}", purity_str, name);

                if node.obstructed
                    && (config.game_phase == crate::models::GamePhase::Phase1
                        || config.game_phase == crate::models::GamePhase::Phase2)
                {
                    *obstructed_nodes.entry(display_name).or_insert(0) += 1;
                } else {
                    *local_nodes.entry(display_name).or_insert(0) += 1;
                }

                let d_m = d_sq_3d.sqrt() / 100.0;
                let decay = decay_weight(d_m, d_m * d_m, config.sigma, config.decay_func);
                let contribution = node_yield_contribution(node, decay, config.game_phase);
                if contribution > 0.0 {
                    *resource_yields.entry(name.clone()).or_insert(0.0) += contribution;
                }
            }
        }

        if d_sq_2d <= tri_radius_sq {
            tri_sum += ((final_z - node.z) / 100.0).abs();
            tri_count += 1;
        }
    }

    if res_to_idx.contains_key("water") {
        let water_yield = virtual_water_yield(curr_x, curr_y, waterwell_nodes, config);
        if water_yield > 0.0 {
            resource_yields.insert("water".to_string(), water_yield);
        }
    }

    let terrain_ruggedness = if tri_count > 0 {
        tri_sum / tri_count as f64
    } else {
        0.0
    };

    let total_yield: f64 = resource_yields.values().sum();
    let diversity_score = if total_yield > 0.0 {
        resource_yields
            .values()
            .filter(|&&y| y > 0.0)
            .map(|&y| {
                let p = y / total_yield;
                -p * p.ln()
            })
            .sum()
    } else {
        0.0
    };

    OptimizationResult {
        x: curr_x,
        y: curr_y,
        z: final_z,
        score: max_score,
        closest_spawn,
        spawn_distance: min_dist / 100.0,
        local_nodes,
        obstructed_nodes,
        resource_yields,
        terrain_ruggedness,
        diversity_score,
    }
}

/// Runs a coarse→fine grid search, then parallelized gradient ascent on top candidates.
struct SearchContext {
    opt_nodes: Vec<OptNode>,
    spatial_grid: SpatialGrid,
    num_resources: usize,
    weights_arr: Vec<f64>,
    epsilons_arr: Vec<f64>,
    res_to_idx: HashMap<String, usize>,
    waterwell_nodes: Vec<(f64, f64)>,
    land_mask: LandMask,
    land_accel: LandAccel,
}

impl SearchContext {
    fn utility(&self, x: f64, y: f64, config: &OptimizerConfig, quality: EvalQuality) -> f64 {
        calculate_utility(
            x,
            y,
            &self.opt_nodes,
            &self.spatial_grid,
            config,
            self.num_resources,
            &self.weights_arr,
            &self.epsilons_arr,
            &self.res_to_idx,
            &self.waterwell_nodes,
            &self.land_mask,
            &self.land_accel,
            quality,
        )
    }

    fn refine_from(
        &self,
        start_x: f64,
        start_y: f64,
        config: &OptimizerConfig,
    ) -> OptimizationResult {
        run_hill_climbing(
            start_x,
            start_y,
            &self.opt_nodes,
            &self.spatial_grid,
            config,
            self.num_resources,
            &self.weights_arr,
            &self.epsilons_arr,
            &self.res_to_idx,
            &self.waterwell_nodes,
            &self.land_mask,
            &self.land_accel,
        )
    }
}

fn spatial_bucket_size_cm(sigma: f64) -> f64 {
    // Match bucket size to the typical query radius (3.5σ), clamped so tiny σ
    // doesn't explode the bucket table and huge σ doesn't go back to 1 km.
    (sigma * 3.5 * 100.0).clamp(20_000.0, 80_000.0)
}

fn prepare_context(nodes: &[ResourceNode], config: &OptimizerConfig) -> SearchContext {
    let mut unique_types: Vec<String> = nodes.iter().map(|n| n.resource_type.clone()).collect();
    for res_name in config.weights.keys() {
        unique_types.push(res_name.clone());
    }
    if config.weights.contains_key("water") {
        unique_types.push("water".to_string());
    }
    unique_types.sort();
    unique_types.dedup();

    let mut res_to_idx = HashMap::new();
    for (i, t) in unique_types.iter().enumerate() {
        res_to_idx.insert(t.clone(), i);
    }

    let num_resources = unique_types.len();
    assert!(
        num_resources <= 128,
        "Too many resource types (max 128 supported by fixed array)"
    );

    let mut weights_arr = vec![0.0; num_resources];
    for (res_name, &weight) in &config.weights {
        if let Some(&idx) = res_to_idx.get(res_name) {
            weights_arr[idx] = weight;
        }
    }

    let leontief_eps = matches!(
        config.utility_func,
        crate::models::UtilityFunction::Leontief
    );
    let mut epsilons_arr = vec![if leontief_eps { 0.001 } else { 0.1 }; num_resources];
    for (i, t) in unique_types.iter().enumerate() {
        epsilons_arr[i] = if leontief_eps {
            0.001
        } else {
            match t.as_str() {
                "iron" | "copper" | "limestone" => 0.005,
                "coal" | "oil" | "waterwell" | "geyser" | "nitrogenwell" => 0.05,
                _ => 0.1,
            }
        };
    }

    let opt_nodes: Vec<OptNode> = nodes
        .iter()
        .map(|n| {
            let multiplier = match config.purity_override {
                crate::models::PurityOverride::Default => n.purity.multiplier(),
                crate::models::PurityOverride::Impure => 0.5,
                crate::models::PurityOverride::Normal => 1.0,
                crate::models::PurityOverride::Pure => 2.0,
            };
            let mut obstructed = n.obstructed;

            if !obstructed && n.resource_type == "caterium" {
                let is_starting_caterium =
                    (n.x - (-220000.0)).abs() < 50000.0 && (n.y - (-150000.0)).abs() < 50000.0;
                if !is_starting_caterium {
                    obstructed = true;
                }
            }

            OptNode {
                x: n.x,
                y: n.y,
                z: n.z,
                res_idx: *res_to_idx.get(&n.resource_type).unwrap(),
                multiplier,
                obstructed,
            }
        })
        .collect();

    let spatial_grid = SpatialGrid::new(&opt_nodes, spatial_bucket_size_cm(config.sigma));
    let waterwell_idx = res_to_idx.get("waterwell").copied();
    let waterwell_nodes = waterwell_idx
        .map(|idx| {
            opt_nodes
                .iter()
                .filter(|node| node.res_idx == idx)
                .map(|node| (node.x, node.y))
                .collect()
        })
        .unwrap_or_default();
    let land_mask = LandMask::from_nodes(&opt_nodes);
    let land_accel = LandAccel::from_mask(&land_mask);

    SearchContext {
        opt_nodes,
        spatial_grid,
        num_resources,
        weights_arr,
        epsilons_arr,
        res_to_idx,
        waterwell_nodes,
        land_mask,
        land_accel,
    }
}

fn pick_diverse_maxima(
    local_maxima: Vec<(f64, f64, f64)>,
    min_dist_between_starts: f64,
    max_candidates: usize,
) -> Vec<(f64, f64, f64)> {
    let mut sorted_maxima = local_maxima;
    sorted_maxima.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    let mut start_candidates: Vec<(f64, f64, f64)> = Vec::new();
    for (x, y, score) in sorted_maxima {
        let is_far_enough = start_candidates.iter().all(|&(cx, cy, _)| {
            let dx = cx - x;
            let dy = cy - y;
            (dx * dx + dy * dy).sqrt() >= min_dist_between_starts
        });
        if is_far_enough {
            start_candidates.push((x, y, score));
            if start_candidates.len() >= max_candidates {
                break;
            }
        }
    }

    if start_candidates.is_empty() {
        for spawn in DEFAULT_SPAWNS {
            start_candidates.push((spawn.x, spawn.y, 0.0));
        }
    }
    start_candidates
}

fn grid_search_refine(
    ctx: &SearchContext,
    config: &OptimizerConfig,
    coarse_res: usize,
    fine_half_window: isize,
    min_dist_between_starts: f64,
    max_candidates: usize,
) -> Vec<OptimizationResult> {
    let step_x = (MAX_X - MIN_X) / coarse_res as f64;
    let step_y = (MAX_Y - MIN_Y) / coarse_res as f64;

    let grid_points: Vec<(f64, f64)> = (0..=coarse_res)
        .flat_map(|row| {
            let y = MIN_Y + row as f64 * step_y;
            (0..=coarse_res).map(move |col| {
                let x = MIN_X + col as f64 * step_x;
                (x, y)
            })
        })
        .collect();

    // Stage 1: coarse screening (no altitude, land accel).
    let scores: Vec<f64> = grid_points
        .into_par_iter()
        .map(|(x, y)| ctx.utility(x, y, config, EvalQuality::Coarse))
        .collect();

    let rows = coarse_res + 1;
    let cols = coarse_res + 1;

    let mut local_maxima = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let idx = r * cols + c;
            let score = scores[idx];
            if score <= 1e-5 {
                continue;
            }

            let mut is_local_max = true;
            'neighbors: for dr in -1..=1 {
                for dc in -1..=1 {
                    if dr == 0 && dc == 0 {
                        continue;
                    }
                    let nr = r as isize + dr;
                    let nc = c as isize + dc;
                    if nr >= 0 && nr < rows as isize && nc >= 0 && nc < cols as isize {
                        let n_idx = (nr as usize) * cols + (nc as usize);
                        if scores[n_idx] > score {
                            is_local_max = false;
                            break 'neighbors;
                        }
                    }
                }
            }

            if is_local_max {
                let x = MIN_X + c as f64 * step_x;
                let y = MIN_Y + r as f64 * step_y;
                local_maxima.push((x, y, score));
            }
        }
    }

    // Coarse local maxima → Fine re-score a wider pool → diversify.
    // Coarse 2D ranking can prefer a different basin than full Fine utility.
    let mut coarse_tops = local_maxima;
    coarse_tops.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    coarse_tops.truncate(max_candidates * 4);

    // Always include spawn + sparse multi-starts so coarse screening cannot
    // drop the basins Fast mode already reaches.
    for spawn in DEFAULT_SPAWNS {
        coarse_tops.push((spawn.x, spawn.y, 0.0));
    }
    let boost_steps = 4;
    let boost_sx = (MAX_X - MIN_X) / (boost_steps + 1) as f64;
    let boost_sy = (MAX_Y - MIN_Y) / (boost_steps + 1) as f64;
    for i in 1..=boost_steps {
        for j in 1..=boost_steps {
            coarse_tops.push((
                MIN_X + i as f64 * boost_sx,
                MIN_Y + j as f64 * boost_sy,
                0.0,
            ));
        }
    }

    let reranked: Vec<(f64, f64, f64)> = coarse_tops
        .into_par_iter()
        .map(|(x, y, _)| (x, y, ctx.utility(x, y, config, EvalQuality::Fine)))
        .collect();
    let start_candidates = pick_diverse_maxima(reranked, min_dist_between_starts, max_candidates);

    // Stage 2: local fine grid around each coarse peak, then gradient ascent.
    let fine_step_x = step_x / (fine_half_window as f64 + 1.0);
    let fine_step_y = step_y / (fine_half_window as f64 + 1.0);

    let refined_results: Vec<OptimizationResult> = start_candidates
        .into_par_iter()
        .map(|(cx, cy, _)| {
            let mut best_x = cx;
            let mut best_y = cy;
            let mut best_score = ctx.utility(cx, cy, config, EvalQuality::Fine);

            for di in -fine_half_window..=fine_half_window {
                for dj in -fine_half_window..=fine_half_window {
                    if di == 0 && dj == 0 {
                        continue;
                    }
                    let fx = (cx + di as f64 * fine_step_x).clamp(MIN_X, MAX_X);
                    let fy = (cy + dj as f64 * fine_step_y).clamp(MIN_Y, MAX_Y);
                    let s = ctx.utility(fx, fy, config, EvalQuality::Fine);
                    if s > best_score {
                        best_score = s;
                        best_x = fx;
                        best_y = fy;
                    }
                }
            }

            ctx.refine_from(best_x, best_y, config)
        })
        .collect();

    top_n_results(refined_results, 3, 150_000.0 / 100.0)
}

/// Returns the top N unique results from a set of refined candidates,
/// filtering out results within min_separation_m metres of a higher-scoring result.
/// Also discards degenerate results where score < min_viable_score (Leontief plateau artifacts).
fn top_n_results(
    mut results: Vec<OptimizationResult>,
    n: usize,
    min_separation_m: f64,
) -> Vec<OptimizationResult> {
    // Save the absolute best result before filtering as a fallback to prevent empty results.
    let absolute_best = results
        .iter()
        .max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();

    // Filter degenerate plateau scores — these occur when hill climbing is seeded
    // at a map corner with no nodes in range, returning only the epsilon floor.
    let min_viable_score = 0.01;
    results.retain(|r| r.score >= min_viable_score);

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<OptimizationResult> = Vec::new();
    for r in results {
        let too_close = kept.iter().any(|k| {
            let dx = (r.x - k.x) / 100.0;
            let dy = (r.y - k.y) / 100.0;
            (dx * dx + dy * dy).sqrt() < min_separation_m
        });
        if !too_close {
            kept.push(r);
            if kept.len() >= n {
                break;
            }
        }
    }

    // Fall back to the single absolute best candidate if all were filtered out.
    if kept.is_empty() {
        if let Some(best) = absolute_best {
            kept.push(best);
        }
    }

    kept
}

fn optimize_hybrid(ctx: &SearchContext, config: &OptimizerConfig) -> Vec<OptimizationResult> {
    // Coarse 150² screen + local fine window ≈ old 500² work with far fewer evals.
    grid_search_refine(ctx, config, 150, 5, 300.0 * 100.0, 50)
}

fn optimize_slow(ctx: &SearchContext, config: &OptimizerConfig) -> Vec<OptimizationResult> {
    grid_search_refine(ctx, config, 250, 6, 200.0 * 100.0, 100)
}

fn optimize_fast(ctx: &SearchContext, config: &OptimizerConfig) -> Vec<OptimizationResult> {
    let mut starts = Vec::new();

    for spawn in DEFAULT_SPAWNS {
        starts.push((spawn.x, spawn.y));
    }

    let steps = 4;
    let step_x = (MAX_X - MIN_X) / (steps + 1) as f64;
    let step_y = (MAX_Y - MIN_Y) / (steps + 1) as f64;
    for i in 1..=steps {
        let x = MIN_X + i as f64 * step_x;
        for j in 1..=steps {
            let y = MIN_Y + j as f64 * step_y;
            starts.push((x, y));
        }
    }

    let refined_results: Vec<OptimizationResult> = starts
        .into_par_iter()
        .map(|(start_x, start_y)| ctx.refine_from(start_x, start_y, config))
        .collect();

    top_n_results(refined_results, 3, 150_000.0 / 100.0)
}

/// Returns up to 5 geographically distinct optimal starting locations, ranked by score.
pub fn optimize(nodes: &[ResourceNode], config: &OptimizerConfig) -> Vec<OptimizationResult> {
    let ctx = prepare_context(nodes, config);
    match config.strategy {
        crate::models::SearchStrategy::Hybrid => optimize_hybrid(&ctx, config),
        crate::models::SearchStrategy::Fast => optimize_fast(&ctx, config),
        crate::models::SearchStrategy::Slow => optimize_slow(&ctx, config),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{GamePhase, OptimizerConfig, Purity, ResourceNode};
    use std::collections::HashMap;

    fn node(resource_type: &str, x: f64, y: f64, obstructed: bool) -> ResourceNode {
        ResourceNode {
            resource_type: resource_type.to_string(),
            purity: Purity::Normal,
            x,
            y,
            z: 0.0,
            obstructed,
        }
    }

    fn config_for(resources: &[(&str, f64)]) -> OptimizerConfig {
        let mut config = OptimizerConfig::default();
        config.weights = HashMap::new();
        for (name, weight) in resources {
            config.weights.insert((*name).to_string(), *weight);
        }
        config.strategy = crate::models::SearchStrategy::Fast;
        config.ignore_spawns = true;
        config
    }

    #[test]
    fn test_water_distance_inside_static_water_body() {
        let distance = distance_to_nearest_water(140000.0, 230000.0, &[]);

        assert!(distance.abs() < f64::EPSILON);
    }

    #[test]
    fn test_water_distance_prefers_nearby_waterwell() {
        let distance = distance_to_nearest_water(0.0, 0.0, &[(300.0, 400.0)]);

        assert!((distance - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_water_distance_without_waterwells_uses_static_water() {
        let distance = distance_to_nearest_water(0.0, 0.0, &[]);
        let expected_distance = (30_000.0_f64.powi(2) + 5_000.0_f64.powi(2)).sqrt() / 100.0;

        assert!((distance - expected_distance).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ignore_spawns() {
        let nodes = crate::data_loader::load_default_nodes();

        let mut config_constrained = OptimizerConfig::default();
        config_constrained.game_phase = GamePhase::Phase1;
        config_constrained.ignore_spawns = false;

        let mut config_ignored = OptimizerConfig::default();
        config_ignored.game_phase = GamePhase::Phase1;
        config_ignored.ignore_spawns = true;

        let ctx = prepare_context(&nodes, &config_constrained);
        let far_dune_desert_x = 291000.0;
        let far_dune_desert_y = 74000.0;

        let constrained_score = ctx.utility(
            far_dune_desert_x,
            far_dune_desert_y,
            &config_constrained,
            EvalQuality::Fine,
        );
        let ignored_score = ctx.utility(
            far_dune_desert_x,
            far_dune_desert_y,
            &config_ignored,
            EvalQuality::Fine,
        );

        assert!(ignored_score > constrained_score);
    }

    #[test]
    fn resource_yields_exclude_early_phase_obstructed_nodes() {
        let nodes = vec![node("iron", 0.0, 0.0, true)];
        let mut config = config_for(&[("iron", 1.0)]);
        config.game_phase = GamePhase::Phase1;
        config.sigma = 500.0;

        let ctx = prepare_context(&nodes, &config);
        let result = ctx.refine_from(0.0, 0.0, &config);

        assert_eq!(result.obstructed_nodes.get("Normal iron"), Some(&1));
        assert_eq!(result.local_nodes.get("Normal iron"), None);
        assert_eq!(
            result.resource_yields.get("iron").copied().unwrap_or(0.0),
            0.0
        );
    }

    #[test]
    fn resource_yields_include_virtual_static_water() {
        let nodes = vec![node("iron", 140000.0, 230000.0, false)];
        let mut config = config_for(&[("iron", 0.1), ("water", 1.0)]);
        config.game_phase = GamePhase::Phase2;
        config.sigma = 500.0;

        let ctx = prepare_context(&nodes, &config);
        let result = ctx.refine_from(140000.0, 230000.0, &config);

        assert!(result.resource_yields.get("water").copied().unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn test_default_nodes_optimize() {
        let nodes = crate::data_loader::load_default_nodes();
        let config = OptimizerConfig::default();
        let results = optimize(&nodes, &config);
        assert!(!results.is_empty());
        assert!(results[0].score > 0.0);
        assert!(!results[0].local_nodes.is_empty());
    }

    #[test]
    fn test_buildable_land_mask_contains_all_nodes() {
        let nodes = crate::data_loader::load_default_nodes();
        let config = OptimizerConfig::default();
        let ctx = prepare_context(&nodes, &config);

        assert_eq!(ctx.land_mask.points.len(), LAND_MASK_SECTORS);
        for node in &ctx.opt_nodes {
            assert!(is_in_polygon(node.x, node.y, &ctx.land_mask.points));
            assert!(
                ctx.land_accel.sample(node.x, node.y).is_some(),
                "land accel should cover node at ({}, {})",
                node.x,
                node.y
            );
        }
    }

    #[test]
    fn land_accel_inland_skips_exact_border_distance() {
        let nodes = crate::data_loader::load_default_nodes();
        let config = OptimizerConfig::default();
        let ctx = prepare_context(&nodes, &config);
        // Grass Fields-ish interior point far from the map polygon edge.
        let x = -50_000.0;
        let y = -50_000.0;
        let field = ctx.land_accel.sample(x, y).expect("interior land");
        assert!(field >= BORDER_MARGIN_CM);
        let penalty = land_border_penalty(x, y, &ctx.land_mask, &ctx.land_accel, EvalQuality::Fine)
            .expect("inland");
        assert!((penalty - 1.0).abs() < f64::EPSILON);
    }
}
