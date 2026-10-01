use satisfactory_start_optimizer::data_loader;
use satisfactory_start_optimizer::models::{self, OptimizerConfig, PurityOverride};
use satisfactory_start_optimizer::optimizer;
use satisfactory_start_optimizer::server;
use std::collections::HashMap;
use std::env;

fn apply_preset_weights(preset_idx: usize, weights: &mut HashMap<String, f64>) {
    if let Some(preset) = models::all_presets().get(preset_idx) {
        *weights = preset.build_weights();
    }
}

fn apply_preset_argument(input: &str, config: &mut OptimizerConfig) -> bool {
    if let Some(preset) = models::preset_by_id_or_phase(input) {
        models::apply_preset_to_config(preset, config);
        true
    } else {
        false
    }
}

fn print_help() {
    println!(
        r#"
FICSIT START OPTIMIZER (RUST VERSION)
--------------------------------------
Calculates the mathematically optimal starting location in Satisfactory 1.0.

Usage:
  satisfactory-start-optimizer [options]

Options:
  --file <path>        Load resource nodes from a custom JSON file
  --sigma <meters>     Logistical walking radius in meters (default: 700)
  --tier <1-5|early|steel|oil|late|quantum>
                       Select game phase preset for non-interactive mode.
  --purity <default|impure|normal|pure>
                       Override database node purity multipliers (default: default)
  --strategy <hybrid|fast|slow>
                       Select search algorithm strategy (default: hybrid)
  --utility <cobbdouglas|leontief|linear>
                       Select utility calculation function (default: cobbdouglas)
  --decay <gaussian|exponential|powerlaw|linear|logisticstep>
                       Select distance decay function (default: gaussian)
  --collectibles       Focus search purely on slugs, drop pods, and alien artifacts
  --ignore-spawns      Ignore distance to starting areas when optimizing
  --server [port]      Start the HTTP API server on port (default: 8080)
  --json               Output only raw JSON optimization results
  --simulate-all       Run the full simulation matrix and write CSV/Markdown reports
  --<resource> <w>     Dynamic weight of any resource type (e.g. --iron 1.5, --uranium -2.0)
  --help               Show this help menu

Interactive terminal UI has been removed. Use the web dashboard instead:
  bun run dev
"#
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return;
    }

    let mut custom_file_path: Option<String> = None;
    let mut config = OptimizerConfig::default();
    if let Some(preset) = models::preset_by_id_or_phase("phase1") {
        models::apply_preset_to_config(preset, &mut config);
    }

    let mut output_json = false;
    let mut run_simulation = false;
    let mut run_server = args.len() == 1;
    let mut server_port: u16 = 8080;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--file" => {
                if i + 1 < args.len() {
                    custom_file_path = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("Error: --file requires a path value");
                    return;
                }
            }
            "--sigma" => {
                if i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<f64>() {
                        if val <= 0.0 {
                            eprintln!("Error: --sigma value must be greater than 0.0");
                            return;
                        }
                        config.sigma = val;
                    } else {
                        eprintln!("Error: --sigma requires a numeric value");
                        return;
                    }
                    i += 2;
                } else {
                    eprintln!("Error: --sigma requires a numeric value");
                    return;
                }
            }
            "--purity" => {
                if i + 1 < args.len() {
                    config.purity_override = match args[i + 1].to_lowercase().as_str() {
                        "impure" => PurityOverride::Impure,
                        "normal" => PurityOverride::Normal,
                        "pure" => PurityOverride::Pure,
                        _ => PurityOverride::Default,
                    };
                    i += 2;
                } else {
                    eprintln!("Error: --purity requires a value");
                    return;
                }
            }
            "--strategy" => {
                if i + 1 < args.len() {
                    config.strategy = match args[i + 1].to_lowercase().as_str() {
                        "fast" => models::SearchStrategy::Fast,
                        "slow" => models::SearchStrategy::Slow,
                        _ => models::SearchStrategy::Hybrid,
                    };
                    i += 2;
                } else {
                    eprintln!("Error: --strategy requires a value");
                    return;
                }
            }
            "--utility" => {
                if i + 1 < args.len() {
                    let val = args[i + 1].to_lowercase();
                    config.utility_func = match val.as_str() {
                        "cobbdouglas" | "cobb-douglas" | "cobb_douglas" => {
                            models::UtilityFunction::CobbDouglas
                        }
                        "leontief" => models::UtilityFunction::Leontief,
                        "linear" => models::UtilityFunction::Linear,
                        _ => {
                            eprintln!(
                                "Error: Invalid utility function value '{}'. Choose from: cobbdouglas, leontief, linear",
                                args[i + 1]
                            );
                            return;
                        }
                    };
                    i += 2;
                } else {
                    eprintln!("Error: --utility requires a value");
                    return;
                }
            }
            "--decay" => {
                if i + 1 < args.len() {
                    let val = args[i + 1].to_lowercase();
                    config.decay_func = match val.as_str() {
                        "gaussian" => models::DistanceDecay::Gaussian,
                        "exponential" => models::DistanceDecay::Exponential,
                        "powerlaw" | "power-law" | "power_law" | "gravity" => {
                            models::DistanceDecay::PowerLaw
                        }
                        "linear" => models::DistanceDecay::Linear,
                        "logisticstep" | "logistic-step" | "step" => {
                            models::DistanceDecay::LogisticStep
                        }
                        _ => {
                            eprintln!(
                                "Error: Invalid decay function value '{}'. Choose from: gaussian, exponential, powerlaw, linear, logisticstep",
                                args[i + 1]
                            );
                            return;
                        }
                    };
                    i += 2;
                } else {
                    eprintln!("Error: --decay requires a value");
                    return;
                }
            }
            "--tier" | "--phase" => {
                if i + 1 < args.len() {
                    let phase_str = &args[i + 1];
                    if !apply_preset_argument(phase_str, &mut config) {
                        eprintln!("Error: Invalid phase/tier value '{}'.", phase_str);
                        return;
                    }
                    i += 2;
                } else {
                    eprintln!("Error: --tier requires a value");
                    return;
                }
            }
            "--collectibles" => {
                apply_preset_argument("collectibles", &mut config);
                i += 1;
            }
            "--ignore-spawns" | "--ignore-starting-areas" => {
                config.ignore_spawns = true;
                i += 1;
            }
            "--server" => {
                run_server = true;
                if i + 1 < args.len() {
                    if let Ok(port) = args[i + 1].parse::<u16>() {
                        server_port = port;
                        i += 2;
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            "--json" => {
                output_json = true;
                run_server = false;
                i += 1;
            }
            "--simulate-all" => {
                run_simulation = true;
                run_server = false;
                i += 1;
            }
            flag if flag.starts_with("--") => {
                let resource_name = flag.trim_start_matches("--").to_string();
                if i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<f64>() {
                        config.weights.insert(resource_name, val);
                    } else {
                        eprintln!("Error: weight value for {} must be a float", flag);
                        return;
                    }
                    i += 2;
                } else {
                    eprintln!("Error: {} requires a numeric value", flag);
                    return;
                }
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                print_help();
                return;
            }
        }
    }

    if run_server {
        if let Err(e) = server::run_server(server_port) {
            eprintln!("API Server error: {:?}", e);
        }
        return;
    }

    let nodes = match &custom_file_path {
        Some(path) => match data_loader::load_nodes_from_file(path) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("Error loading JSON file: {}", e);
                return;
            }
        },
        None => data_loader::load_default_nodes(),
    };

    if run_simulation {
        run_full_simulation_matrix(&nodes);
        return;
    }

    let result = optimizer::optimize(&nodes, &config);
    if output_json {
        if let Ok(json_str) = serde_json::to_string_pretty(&result) {
            println!("{}", json_str);
        }
    } else {
        print_help();
        eprintln!("\nNo terminal UI is available. Start the web dashboard with: bun run dev");
    }
}

fn run_full_simulation_matrix(nodes: &[models::ResourceNode]) {
    use std::fs::File;
    use std::io::Write;

    struct SimConfig {
        preset_idx: usize,
        purity: PurityOverride,
        utility: models::UtilityFunction,
        decay: models::DistanceDecay,
        sigma: f64,
    }

    let mut sim_configs = Vec::new();
    let presets = 0..models::all_presets().len();
    let purities = [
        PurityOverride::Default,
        PurityOverride::Normal,
        PurityOverride::Pure,
    ];
    let utilities = [
        models::UtilityFunction::CobbDouglas,
        models::UtilityFunction::Leontief,
        models::UtilityFunction::Linear,
    ];
    let decays = [
        models::DistanceDecay::Gaussian,
        models::DistanceDecay::Exponential,
        models::DistanceDecay::PowerLaw,
        models::DistanceDecay::Linear,
        models::DistanceDecay::LogisticStep,
    ];

    for preset_idx in presets {
        for &purity in &purities {
            for &utility in &utilities {
                for &decay in &decays {
                    let preset = models::all_presets()[preset_idx];
                    sim_configs.push(SimConfig {
                        preset_idx,
                        purity,
                        utility,
                        decay,
                        sigma: preset.sigma,
                    });
                }
            }
        }
    }

    println!(
        "Running {} simulation configurations sequentially (each internally parallelized)...",
        sim_configs.len()
    );
    let results: Vec<(SimConfig, optimizer::OptimizationResult)> = sim_configs
        .into_iter()
        .map(|config| {
            let preset = models::all_presets()[config.preset_idx];
            let mut opt_config = OptimizerConfig {
                sigma: config.sigma,
                weights: HashMap::new(),
                purity_override: config.purity,
                strategy: models::SearchStrategy::Hybrid,
                utility_func: config.utility,
                decay_func: config.decay,
                game_phase: preset.phase,
                ignore_spawns: true,
            };
            apply_preset_weights(config.preset_idx, &mut opt_config.weights);
            let mut all_res = optimizer::optimize(nodes, &opt_config);
            let res = all_res.remove(0);
            (config, res)
        })
        .collect();

    let csv_path = "simulation_results.csv";
    let mut file = File::create(csv_path).expect("Failed to create CSV file");
    writeln!(
        file,
        "Preset,Purity,Utility,Decay,Radius,X,Y,Z,Score,Spawn,SpawnDistance"
    )
    .unwrap();
    for (conf, res) in &results {
        let preset_name = match conf.preset_idx {
            0 => "Phase 1: Early Game",
            1 => "Phase 2: Steel & Coal",
            2 => "Phase 3: Oil & Quartz",
            3 => "Phase 4: Late Game",
            4 => "Phase 5: Quantum",
            5 => "Collectible Hunting",
            _ => "Unknown",
        };
        let purity_str = match conf.purity {
            PurityOverride::Default => "Default",
            PurityOverride::Impure => "Impure",
            PurityOverride::Normal => "Normal",
            PurityOverride::Pure => "Pure",
        };
        writeln!(
            file,
            "\"{}\",\"{}\",\"{}\",\"{}\",{},{:.2},{:.2},{:.2},{:.4},\"{}\",{:.2}",
            preset_name,
            purity_str,
            conf.utility.to_str(),
            conf.decay.to_str(),
            conf.sigma,
            res.x,
            res.y,
            res.z,
            res.score,
            res.closest_spawn.name,
            res.spawn_distance
        )
        .unwrap();
    }
    println!(
        "Saved raw simulation dataset ({} rows) to {}",
        results.len(),
        csv_path
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_phase_preset_application_sets_radius_and_spawn_behavior() {
        let mut phase1_config = OptimizerConfig::default();
        assert!(apply_preset_argument("phase1", &mut phase1_config));
        assert_eq!(phase1_config.sigma, 200.0);
        assert!(!phase1_config.ignore_spawns);

        let mut phase4_config = OptimizerConfig::default();
        assert!(apply_preset_argument("phase4", &mut phase4_config));
        assert_eq!(phase4_config.sigma, 600.0);
        assert!(phase4_config.ignore_spawns);
    }

    #[test]
    fn cli_collectibles_preset_application_uses_shared_weights() {
        let mut config = OptimizerConfig::default();
        assert!(apply_preset_argument("collectibles", &mut config));

        assert_eq!(config.sigma, 1000.0);
        assert!(config.ignore_spawns);
        assert_eq!(config.weights.get("blueslug").copied(), Some(0.3));
        assert_eq!(config.weights.get("harddrive").copied(), Some(1.5));
    }
}
