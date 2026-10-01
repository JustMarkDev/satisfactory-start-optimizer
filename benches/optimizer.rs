use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use satisfactory_start_optimizer::data_loader;
use satisfactory_start_optimizer::models::{
    DistanceDecay, OptimizerConfig, SearchStrategy, UtilityFunction, apply_preset_to_config,
    preset_by_id_or_phase,
};
use satisfactory_start_optimizer::optimizer;
use std::hint::black_box;
use std::time::Duration;

fn phase_config(preset_id: &str, strategy: SearchStrategy) -> OptimizerConfig {
    let mut config = OptimizerConfig::default();
    let preset = preset_by_id_or_phase(preset_id).expect("known preset");
    apply_preset_to_config(preset, &mut config);
    config.strategy = strategy;
    config
}

fn bench_map_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("map_load");
    group.throughput(Throughput::Bytes(
        include_bytes!("../public/data/complete-map-data.json").len() as u64,
    ));
    group.bench_function("load_default_nodes", |b| {
        b.iter(|| {
            let nodes = data_loader::load_default_nodes();
            black_box(nodes.len())
        });
    });
    group.finish();
}

fn bench_search_strategies(c: &mut Criterion) {
    let nodes = data_loader::load_default_nodes();
    let mut group = c.benchmark_group("search_strategy");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(30));
    group.throughput(Throughput::Elements(nodes.len() as u64));

    for (label, strategy) in [
        ("fast", SearchStrategy::Fast),
        ("hybrid", SearchStrategy::Hybrid),
        ("slow", SearchStrategy::Slow),
    ] {
        let config = phase_config("phase1", strategy);
        group.bench_with_input(BenchmarkId::new("phase1", label), &config, |b, config| {
            b.iter(|| {
                let results = optimizer::optimize(black_box(&nodes), black_box(config));
                black_box(results[0].score)
            });
        });
    }
    group.finish();
}

fn bench_utility_functions(c: &mut Criterion) {
    let nodes = data_loader::load_default_nodes();
    let mut group = c.benchmark_group("utility_function");
    group.sample_size(15);
    group.measurement_time(Duration::from_secs(25));

    for (label, utility) in [
        ("cobb_douglas", UtilityFunction::CobbDouglas),
        ("leontief", UtilityFunction::Leontief),
        ("linear", UtilityFunction::Linear),
    ] {
        let mut config = phase_config("phase1", SearchStrategy::Hybrid);
        config.utility_func = utility;
        group.bench_with_input(BenchmarkId::from_parameter(label), &config, |b, config| {
            b.iter(|| {
                let results = optimizer::optimize(black_box(&nodes), black_box(config));
                black_box(results[0].score)
            });
        });
    }
    group.finish();
}

fn bench_distance_decay(c: &mut Criterion) {
    let nodes = data_loader::load_default_nodes();
    let mut group = c.benchmark_group("distance_decay");
    group.sample_size(15);
    group.measurement_time(Duration::from_secs(25));

    for (label, decay) in [
        ("gaussian", DistanceDecay::Gaussian),
        ("exponential", DistanceDecay::Exponential),
        ("power_law", DistanceDecay::PowerLaw),
        ("linear", DistanceDecay::Linear),
        ("logistic_step", DistanceDecay::LogisticStep),
    ] {
        let mut config = phase_config("phase1", SearchStrategy::Hybrid);
        config.decay_func = decay;
        group.bench_with_input(BenchmarkId::from_parameter(label), &config, |b, config| {
            b.iter(|| {
                let results = optimizer::optimize(black_box(&nodes), black_box(config));
                black_box(results[0].score)
            });
        });
    }
    group.finish();
}

fn bench_game_phases(c: &mut Criterion) {
    let nodes = data_loader::load_default_nodes();
    let mut group = c.benchmark_group("game_phase");
    group.sample_size(15);
    group.measurement_time(Duration::from_secs(25));

    for preset_id in ["phase1", "phase2", "phase3", "phase4", "phase5", "collectibles"] {
        let config = phase_config(preset_id, SearchStrategy::Hybrid);
        group.bench_with_input(BenchmarkId::from_parameter(preset_id), &config, |b, config| {
            b.iter(|| {
                let results = optimizer::optimize(black_box(&nodes), black_box(config));
                black_box(results[0].score)
            });
        });
    }
    group.finish();
}

fn bench_thread_scaling(c: &mut Criterion) {
    let nodes = data_loader::load_default_nodes();
    let config = phase_config("phase1", SearchStrategy::Hybrid);
    let mut group = c.benchmark_group("thread_scaling");
    group.sample_size(12);
    group.measurement_time(Duration::from_secs(30));

    for threads in [1usize, 2, 4, 8] {
        group.bench_with_input(BenchmarkId::new("hybrid_phase1", threads), &threads, |b, &n| {
            b.iter_custom(|iters| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build()
                    .expect("rayon pool");
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let start = std::time::Instant::now();
                    pool.install(|| {
                        let results = optimizer::optimize(black_box(&nodes), black_box(&config));
                        black_box(results[0].score);
                    });
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_map_load,
    bench_search_strategies,
    bench_utility_functions,
    bench_distance_decay,
    bench_game_phases,
    bench_thread_scaling,
);
criterion_main!(benches);
