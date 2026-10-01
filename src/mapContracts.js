// Resource Registry and Stylings
export const RESOURCES = [
  // Core Resources
  { id: "iron", name: "Iron Ore", color: "#4682B4", category: "core" },
  { id: "copper", name: "Copper Ore", color: "#B87333", category: "core" },
  { id: "limestone", name: "Limestone", color: "#E5E5C0", category: "core" },
  { id: "coal", name: "Coal", color: "#555555", category: "core" },
  { id: "water", name: "Water (Static)", color: "#00BFFF", category: "core" },
  { id: "oil", name: "Crude Oil", color: "#2E2E2E", category: "core" },
  { id: "sulfur", name: "Sulfur", color: "#DAA520", category: "core" },
  { id: "quartz", name: "Raw Quartz", color: "#FFD1DC", category: "core" },
  { id: "caterium", name: "Caterium Ore", color: "#FFD700", category: "core" },
  { id: "bauxite", name: "Bauxite", color: "#CD5C5C", category: "core" },
  { id: "uranium", name: "Uranium", color: "#7FFF00", category: "core" },
  { id: "sam", name: "SAM Ore", color: "#9370DB", category: "core" },
  { id: "nitrogenwell", name: "Nitrogen Gas Well", color: "#48D1CC", category: "core" },
  { id: "waterwell", name: "Water Well", color: "#1E90FF", category: "core" },
  { id: "geyser", name: "Geyser", color: "#A9A9A9", category: "core" },

  // Collectibles / Research
  { id: "blueslug", name: "Blue Slug", color: "#00FFFF", category: "collectible" },
  { id: "yellowslug", name: "Yellow Slug", color: "#FFFF00", category: "collectible" },
  { id: "purpleslug", name: "Purple Slug", color: "#FF00FF", category: "collectible" },
  { id: "mercer", name: "Mercer Sphere", color: "#FF7F50", category: "collectible" },
  { id: "somersloop", name: "Somersloop", color: "#D1C4E9", category: "collectible" },
  { id: "harddrive", name: "Hard Drive", color: "#CD7F32", category: "collectible" },
  { id: "paleberry", name: "Paleberry", color: "#DB7093", category: "collectible" },
  { id: "berylnut", name: "Berylnut", color: "#D2B48C", category: "collectible" },
  { id: "baconagaric", name: "Bacon Agaric", color: "#BC8F8F", category: "collectible" },

  // Threats
  { id: "sporeflower", name: "Spore Flower", color: "#8B008B", category: "threat" },
  { id: "gaspillar", name: "Gas Pillar", color: "#2E8B57", category: "threat" },
];

export const DEFAULT_PHASE_IDS = ["phase1", "phase2", "phase3", "phase4", "phase5", "collectibles"];

export function parsePurityMultiplier(purity) {
  const normalizedPurity = String(purity).toLowerCase();

  if (normalizedPurity === "rp_pure" || normalizedPurity === "pure") {
    return 2.0;
  }
  if (
    normalizedPurity === "rp_impure" ||
    normalizedPurity === "rp_inpure" ||
    normalizedPurity === "impure" ||
    normalizedPurity === "inpure"
  ) {
    return 0.5;
  }
  return 1.0;
}

export function distanceMeters(a, b) {
  const dx = Number(a?.x) - Number(b?.x);
  const dy = Number(a?.y) - Number(b?.y);
  if (!Number.isFinite(dx) || !Number.isFinite(dy)) return Number.POSITIVE_INFINITY;

  return Math.sqrt(dx * dx + dy * dy) / 100;
}

export function effectiveNodeYield(node) {
  const explicitMultiplier = Number(node?.purityMultiplier);
  if (Number.isFinite(explicitMultiplier)) return explicitMultiplier;

  return parsePurityMultiplier(node?.purity);
}

export function nodeInspectionKey(node) {
  return `${node?.resource_type}:${node?.x}:${node?.y}:${node?.z ?? ""}`;
}

export function purityLabel(node) {
  const yieldValue = effectiveNodeYield(node);
  if (yieldValue > 1.5) return "Pure";
  if (yieldValue < 0.8) return "Impure";
  return "Normal";
}

export function nonZeroWeights(weights) {
  return Object.fromEntries(Object.entries(weights).filter(([, value]) => value !== 0));
}

export function hasOptimizationObjective(weights) {
  return Object.keys(nonZeroWeights(weights)).length > 0;
}

export function resourceMeta(resourceId) {
  return (
    RESOURCES.find((resource) => resource.id === resourceId) || {
      id: resourceId,
      name: resourceId,
      color: "#9aa0a6",
      category: "unknown",
    }
  );
}

export function nearbyWeightedNodes(rawNodes, selectedResult, weights, sigma, limit = 8) {
  if (!Array.isArray(rawNodes) || !selectedResult || !weights || typeof weights !== "object") {
    return [];
  }

  const radiusMeters = Number(sigma);
  if (!Number.isFinite(radiusMeters) || radiusMeters < 0) return [];

  const maxRows = Number.isFinite(limit)
    ? Math.max(0, Math.floor(limit))
    : Number.POSITIVE_INFINITY;

  return rawNodes
    .map((node) => {
      const resourceId = node?.resource_type;
      const weight = Number(weights[resourceId] || 0);
      if (weight === 0) return null;

      const distance = distanceMeters(node, selectedResult);
      if (!Number.isFinite(distance) || distance > radiusMeters) return null;

      const yieldValue = effectiveNodeYield(node);
      const contribution = weight * yieldValue;
      const meta = resourceMeta(resourceId);

      return {
        key: nodeInspectionKey(node),
        id: resourceId,
        resourceId,
        name: meta.name,
        color: meta.color,
        category: meta.category,
        distance,
        purity: purityLabel(node),
        yield: yieldValue,
        obstructed: Boolean(node?.obstructed),
        contribution,
      };
    })
    .filter(Boolean)
    .sort(
      (a, b) =>
        b.contribution - a.contribution || a.distance - b.distance || a.id.localeCompare(b.id),
    )
    .slice(0, maxRows);
}

export function formatYield(value) {
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return "0.00";

  return Math.abs(numericValue) < 10 ? numericValue.toFixed(2) : numericValue.toFixed(1);
}

export function topResourceYields(resourceYields = {}, limit = 4) {
  if (!resourceYields || typeof resourceYields !== "object") return [];

  return Object.entries(resourceYields)
    .map(([id, value]) => ({ id, value: Number(value) }))
    .filter((yieldEntry) => Number.isFinite(yieldEntry.value) && yieldEntry.value > 0)
    .sort((a, b) => b.value - a.value || a.id.localeCompare(b.id))
    .slice(0, limit)
    .map(({ id, value }) => {
      const meta = resourceMeta(id);
      return {
        id,
        name: meta.name,
        color: meta.color,
        value,
      };
    });
}

export function countObstructedNodes(obstructedNodes = {}) {
  if (!obstructedNodes || typeof obstructedNodes !== "object") return 0;

  if (Array.isArray(obstructedNodes)) return obstructedNodes.length;

  return Object.values(obstructedNodes).reduce((total, count) => {
    const numericCount = Number(count);
    return Number.isFinite(numericCount) && numericCount > 0 ? total + numericCount : total;
  }, 0);
}

export function formatRuggedness(value) {
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return "0.0m";

  return `${numericValue.toFixed(1)}m`;
}

export function formatDiversity(value) {
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return "0.00";

  return numericValue.toFixed(2);
}
