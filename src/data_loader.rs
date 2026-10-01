use crate::models::{Purity, ResourceNode};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

const EMBEDDED_NODES: &str = include_str!("../public/data/complete-map-data.json");

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MapDataRoot {
    options: Vec<Category>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Category {
    name: Option<String>,
    options: Vec<SubCategory>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SubCategory {
    name: Option<String>,
    options: Vec<Item>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Item {
    #[serde(rename = "layerId")]
    layer_id: String,
    name: Option<String>,
    purity: Option<String>,
    markers: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Marker {
    #[serde(rename = "pathName")]
    path_name: Option<String>,
    x: f64,
    y: f64,
    z: Option<f64>,
    #[serde(rename = "type")]
    marker_type: Option<String>,
    purity: Option<String>,
    obstructed: Option<bool>,
}

pub fn load_nodes_from_str(s: &str) -> Result<Vec<ResourceNode>, Box<dyn std::error::Error>> {
    // Try parsing as new format
    let root_res = serde_json::from_str::<MapDataRoot>(s);
    match root_res {
        Ok(root) => {
            let mut nodes = Vec::new();
            for category in root.options {
                for subcat in category.options {
                    for item in subcat.options {
                        let default_res_type = match item.layer_id.as_str() {
                            id if id.starts_with("limestone") => "limestone",
                            id if id.starts_with("iron") => "iron",
                            id if id.starts_with("copper") => "copper",
                            id if id.starts_with("caterium") => "caterium",
                            id if id.starts_with("coal") => "coal",
                            id if id.starts_with("oilWell") => "oil",
                            id if id.starts_with("oil") => "oil",
                            id if id.starts_with("sulfur") => "sulfur",
                            id if id.starts_with("bauxite") => "bauxite",
                            id if id.starts_with("quartz") => "quartz",
                            id if id.starts_with("uranium") => "uranium",
                            id if id.starts_with("sam") => "sam",
                            id if id.starts_with("nitrogen") => "nitrogenwell",
                            id if id.starts_with("water") => "waterwell",
                            id if id.starts_with("geyser") => "geyser",
                            "greenSlugs" => "blueslug",
                            "yellowSlugs" => "yellowslug",
                            "purpleSlugs" => "purpleslug",
                            "mercerSpheres" => "mercer",
                            "somersloops" => "somersloop",
                            "hardDrives" => "harddrive",
                            "paleBerry" => "paleberry",
                            "berylNut" => "berylnut",
                            "baconAgaric" => "baconagaric",
                            "sporeFlowers" => "sporeflower",
                            "pillars" => "gaspillar",
                            _ => "",
                        };

                        if default_res_type.is_empty() {
                            continue;
                        }

                        if let Some(markers_val) = item.markers {
                            match serde_json::from_value::<Vec<Marker>>(markers_val) {
                                Ok(markers_list) => {
                                    for marker in markers_list {
                                        let res_type = match marker.marker_type.as_deref() {
                                            Some("Desc_Stone_C") => "limestone",
                                            Some("Desc_OreIron_C") => "iron",
                                            Some("Desc_OreCopper_C") => "copper",
                                            Some("Desc_OreGold_C") => "caterium",
                                            Some("Desc_Coal_C") => "coal",
                                            Some("Desc_LiquidOil_C") => "oil",
                                            Some("Desc_Sulfur_C") => "sulfur",
                                            Some("Desc_OreBauxite_C") => "bauxite",
                                            Some("Desc_RawQuartz_C") => "quartz",
                                            Some("Desc_OreUranium_C") => "uranium",
                                            Some("Desc_SAM_C") => "sam",
                                            Some("Desc_NitrogenGas_C") => "nitrogenwell",
                                            Some("Desc_Water_C") => "waterwell",
                                            _ => {
                                                if let Some(ref path) = marker.path_name {
                                                    if path.contains("BP_ResourceNodeGeyser") {
                                                        "geyser"
                                                    } else {
                                                        default_res_type
                                                    }
                                                } else {
                                                    default_res_type
                                                }
                                            }
                                        };

                                        let purity_str =
                                            marker.purity.as_deref().unwrap_or("RP_Normal");
                                        let purity =
                                            Purity::from_str(purity_str).unwrap_or(Purity::Normal);

                                        nodes.push(ResourceNode {
                                            resource_type: res_type.to_string(),
                                            purity,
                                            x: marker.x,
                                            y: marker.y,
                                            z: marker.z.unwrap_or(0.0),
                                            obstructed: marker.obstructed.unwrap_or(false),
                                        });
                                    }
                                }
                                Err(e) => {
                                    eprintln!(
                                        "Error parsing markers for item {}: {:?}",
                                        item.layer_id, e
                                    );
                                }
                            }
                        }
                    }
                }
            }
            return Ok(nodes);
        }
        Err(err) => {
            eprintln!("Error parsing new format: {:?}", err);
        }
    }

    // Try parsing as old format
    let old_res = serde_json::from_str::<Vec<ResourceNode>>(s);
    match old_res {
        Ok(nodes) => return Ok(nodes),
        Err(err) => {
            eprintln!("Error parsing old format: {:?}", err);
        }
    }

    Err("Failed to parse resource nodes JSON in either new or old format".into())
}

pub fn load_default_nodes() -> Vec<ResourceNode> {
    match load_nodes_from_str(EMBEDDED_NODES) {
        Ok(n) => n,
        Err(e) => panic!("Failed to parse embedded resource nodes JSON: {:?}", e),
    }
}

pub fn load_nodes_from_file<P: AsRef<Path>>(
    path: P,
) -> Result<Vec<ResourceNode>, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    load_nodes_from_str(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Purity;

    #[test]
    fn parses_new_map_format_marker() {
        let nodes = load_nodes_from_str(
            r#"{
                "options": [
                    {
                        "options": [
                            {
                                "options": [
                                    {
                                        "layerId": "ironNodes",
                                        "markers": [
                                            {
                                                "type": "Desc_OreIron_C",
                                                "x": 123.5,
                                                "y": -456.25,
                                                "z": 78.0,
                                                "purity": "RP_Pure",
                                                "obstructed": true
                                            }
                                        ]
                                    }
                                ]
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("new map format should parse");

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].resource_type, "iron");
        assert_eq!(nodes[0].purity, Purity::Pure);
        assert_eq!(nodes[0].x, 123.5);
        assert_eq!(nodes[0].y, -456.25);
        assert_eq!(nodes[0].z, 78.0);
        assert!(nodes[0].obstructed);
    }

    #[test]
    fn parses_old_resource_node_array_format() {
        let nodes = load_nodes_from_str(
            r#"[
                {
                    "type": "copper",
                    "purity": "normal",
                    "x": 1.0,
                    "y": 2.0,
                    "z": 3.0,
                    "obstructed": false
                }
            ]"#,
        )
        .expect("old node array format should parse");

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].resource_type, "copper");
        assert_eq!(nodes[0].purity, Purity::Normal);
        assert_eq!(nodes[0].x, 1.0);
        assert_eq!(nodes[0].y, 2.0);
        assert_eq!(nodes[0].z, 3.0);
        assert!(!nodes[0].obstructed);
    }

    #[test]
    fn parses_engine_purity_tokens() {
        let fixture = |purity: &str| {
            format!(
                r#"{{
                    "options": [
                        {{
                            "options": [
                                {{
                                    "options": [
                                        {{
                                            "layerId": "limestoneNodes",
                                            "markers": [
                                                {{
                                                    "type": "Desc_Stone_C",
                                                    "x": 0.0,
                                                    "y": 0.0,
                                                    "purity": "{purity}"
                                                }}
                                            ]
                                        }}
                                    ]
                                }}
                            ]
                        }}
                    ]
                }}"#
            )
        };

        let impure = load_nodes_from_str(&fixture("RP_Inpure")).expect("impure token should parse");
        let normal = load_nodes_from_str(&fixture("RP_Normal")).expect("normal token should parse");
        let pure = load_nodes_from_str(&fixture("RP_Pure")).expect("pure token should parse");

        assert_eq!(impure[0].purity, Purity::Impure);
        assert_eq!(normal[0].purity, Purity::Normal);
        assert_eq!(pure[0].purity, Purity::Pure);
    }
}
