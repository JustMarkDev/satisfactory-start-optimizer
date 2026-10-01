import { describe, expect, test } from "vite-plus/test";

import {
  countObstructedNodes,
  DEFAULT_PHASE_IDS,
  formatDiversity,
  formatRuggedness,
  formatYield,
  hasOptimizationObjective,
  nearbyWeightedNodes,
  nonZeroWeights,
  parsePurityMultiplier,
  RESOURCES,
  resourceMeta,
  topResourceYields,
} from "./mapContracts.js";

describe("map contracts", () => {
  test("resource IDs are unique", () => {
    const ids = RESOURCES.map((resource) => resource.id);

    expect(new Set(ids).size).toBe(ids.length);
  });

  test("default UI phases match API preset IDs", () => {
    expect(DEFAULT_PHASE_IDS).toEqual([
      "phase1",
      "phase2",
      "phase3",
      "phase4",
      "phase5",
      "collectibles",
    ]);
  });

  test("parsePurityMultiplier maps engine purity tokens", () => {
    expect(parsePurityMultiplier("RP_Inpure")).toBe(0.5);
    expect(parsePurityMultiplier("RP_Impure")).toBe(0.5);
    expect(parsePurityMultiplier("RP_Normal")).toBe(1.0);
    expect(parsePurityMultiplier("RP_Pure")).toBe(2.0);
  });

  test("parsePurityMultiplier maps lowercase purity tokens", () => {
    expect(parsePurityMultiplier("impure")).toBe(0.5);
    expect(parsePurityMultiplier("normal")).toBe(1.0);
    expect(parsePurityMultiplier("pure")).toBe(2.0);
  });

  test("empty optimization objectives are detectable before API calls", () => {
    expect(nonZeroWeights({ iron: 0, copper: 1 })).toEqual({ copper: 1 });
    expect(hasOptimizationObjective({ iron: 0, copper: 0 })).toBe(false);
    expect(hasOptimizationObjective({ iron: 0, copper: 1 })).toBe(true);
  });

  test("resourceMeta maps resource IDs to display metadata", () => {
    expect(resourceMeta("iron")).toMatchObject({
      id: "iron",
      name: "Iron Ore",
      color: "#4682B4",
    });
  });

  test("resourceMeta returns safe fallback metadata for unknown IDs", () => {
    expect(resourceMeta("<script>alert(1)</script>")).toEqual({
      id: "<script>alert(1)</script>",
      name: "<script>alert(1)</script>",
      color: "#9aa0a6",
      category: "unknown",
    });
  });

  test("topResourceYields sorts descending and limits results", () => {
    expect(topResourceYields({ copper: 3, iron: 12, limestone: 7, coal: 0 }, 2)).toEqual([
      { id: "iron", name: "Iron Ore", color: "#4682B4", value: 12 },
      { id: "limestone", name: "Limestone", color: "#E5E5C0", value: 7 },
    ]);
  });

  test("topResourceYields ignores empty, missing, and non-positive maps", () => {
    expect(topResourceYields()).toEqual([]);
    expect(topResourceYields({ iron: 0, copper: -1, limestone: Number.NaN })).toEqual([]);
  });

  test("countObstructedNodes sums map counts with defensive fallbacks", () => {
    expect(countObstructedNodes({ "Normal iron": 2, "Impure copper": 1 })).toBe(3);
    expect(
      countObstructedNodes({ "Normal iron": 0, "Impure copper": -1, limestone: Number.NaN }),
    ).toBe(0);
    expect(countObstructedNodes()).toBe(0);
  });

  test("countObstructedNodes still handles legacy array payloads", () => {
    expect(countObstructedNodes(["Normal iron", "Impure copper"])).toBe(2);
  });

  test("result explanation numbers use compact labels", () => {
    expect(formatYield(3.456)).toBe("3.46");
    expect(formatYield(12.34)).toBe("12.3");
    expect(formatRuggedness(4.44)).toBe("4.4m");
    expect(formatDiversity(0.678)).toBe("0.68");
  });

  test("nearbyWeightedNodes excludes nodes outside sigma", () => {
    const rows = nearbyWeightedNodes(
      [
        { resource_type: "iron", x: 5_000, y: 0, purityMultiplier: 1 },
        { resource_type: "copper", x: 25_000, y: 0, purityMultiplier: 1 },
      ],
      { x: 0, y: 0 },
      { iron: 1, copper: 1 },
      100,
      8,
    );

    expect(rows.map((row) => row.id)).toEqual(["iron"]);
    expect(rows[0].distance).toBe(50);
  });

  test("nearbyWeightedNodes excludes zero-weight resources", () => {
    const rows = nearbyWeightedNodes(
      [
        { resource_type: "iron", x: 0, y: 0, purityMultiplier: 1 },
        { resource_type: "copper", x: 0, y: 0, purityMultiplier: 1 },
      ],
      { x: 0, y: 0 },
      { iron: 1, copper: 0 },
      100,
      8,
    );

    expect(rows.map((row) => row.id)).toEqual(["iron"]);
  });

  test("nearbyWeightedNodes keeps negative threat contribution and metadata", () => {
    const rows = nearbyWeightedNodes(
      [{ resource_type: "gaspillar", x: 0, y: 0, purityMultiplier: 1 }],
      { x: 0, y: 0 },
      { gaspillar: -1.5 },
      100,
      8,
    );

    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      id: "gaspillar",
      name: "Gas Pillar",
      category: "threat",
      contribution: -1.5,
    });
  });

  test("nearbyWeightedNodes orders contributions by purity multiplier", () => {
    const rows = nearbyWeightedNodes(
      [
        { resource_type: "iron", x: 0, y: 0, purity: "RP_Inpure" },
        { resource_type: "copper", x: 100, y: 0, purity: "RP_Normal" },
        { resource_type: "limestone", x: 200, y: 0, purity: "RP_Pure" },
      ],
      { x: 0, y: 0 },
      { iron: 1, copper: 1, limestone: 1 },
      100,
      8,
    );

    expect(rows.map((row) => row.id)).toEqual(["limestone", "copper", "iron"]);
    expect(rows.map((row) => row.contribution)).toEqual([2, 1, 0.5]);
  });

  test("nearbyWeightedNodes preserves obstructed state", () => {
    const rows = nearbyWeightedNodes(
      [{ resource_type: "caterium", x: 0, y: 0, purityMultiplier: 1, obstructed: true }],
      { x: 0, y: 0 },
      { caterium: 1 },
      100,
      8,
    );

    expect(rows[0].obstructed).toBe(true);
  });
});
