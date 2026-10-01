// FICSIT Starting Position Optimizer
// Web Dashboard Logic

import {
  countObstructedNodes,
  formatDiversity,
  formatRuggedness,
  formatYield,
  hasOptimizationObjective,
  nearbyWeightedNodes,
  nodeInspectionKey,
  nonZeroWeights,
  RESOURCES,
  topResourceYields,
} from "./mapContracts.js";

const LAND_MASK_SECTORS = 128;
const LAND_MASK_BUFFER_CM = 22000;
const MAP_PIXEL_TO_CM = 1 / 0.0013653321;

let DEFAULT_SPAWNS = [];
let PRESETS = {};
let PRESETS_RAW = [];
let buildableLandPolygon = [];

// App State
const state = {
  rawNodes: [],
  config: {
    sigma: 200,
    utilityFunc: "cobb_douglas",
    decayFunc: "gaussian",
    purityOverride: "default",
    ignoreSpawns: false,
    gamePhase: "phase1",
    strategy: "hybrid",
    weights: {},
  },
  selectedResultIdx: 0,
  results: [],
  mapType: "realistic", // "game" or "realistic"
  visibleLayers: {
    nodes: true,
  },
};

// UI Elements
const els = {
  mapImg: document.getElementById("map-img"),
  svgOverlay: document.getElementById("svg-overlay"),
  resultsList: document.getElementById("results-list"),
  weightsContainer: document.getElementById("weights-container"),
  mapLoading: document.getElementById("map-loading"),
  mapTooltip: document.getElementById("map-tooltip"),
  zoomContainer: document.getElementById("zoom-container"),
  mapInnerContainer: document.getElementById("map-inner-container"),

  paramUtility: document.getElementById("param-utility"),
  paramDecay: document.getElementById("param-decay"),
  paramPurity: document.getElementById("param-purity"),
  paramStrategy: document.getElementById("param-strategy"),
  paramSigma: document.getElementById("param-sigma"),
  paramSigmaValue: document.getElementById("param-sigma-value"),
  paramIgnoreSpawns: document.getElementById("param-ignore-spawns"),
  btnCompute: document.getElementById("btn-compute"),
};

// Projection conversion formulas (Game coordinate -> Raster pixel coordinates inside a 1280x1280 canvas/SVG)
function gameToPixel(gx, gy) {
  const px = 571.321 + gx * 0.0013653321;
  const py = 640.0 + gy * 0.0013653333;
  return { x: px, y: py };
}

function formatContribution(value) {
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return "0.00";

  const formatted = formatYield(numericValue);
  return numericValue > 0 ? `+${formatted}` : formatted;
}

function computeBuildableLandPolygon(nodes) {
  if (!nodes.length) return [];

  const center = nodes.reduce(
    (acc, node) => {
      acc.x += node.x;
      acc.y += node.y;
      return acc;
    },
    { x: 0, y: 0 },
  );
  center.x /= nodes.length;
  center.y /= nodes.length;

  const radii = Array.from({ length: LAND_MASK_SECTORS }, () => 0);
  nodes.forEach((node) => {
    const dx = node.x - center.x;
    const dy = node.y - center.y;
    const dist = Math.sqrt(dx * dx + dy * dy);
    if (dist === 0) return;

    const angle = Math.atan2(dy, dx);
    const normalized = (angle + Math.PI) / (2 * Math.PI);
    const idx = Math.min(LAND_MASK_SECTORS - 1, Math.floor(normalized * LAND_MASK_SECTORS));
    radii[idx] = Math.max(radii[idx], dist);
  });

  const originalRadii = [...radii];
  for (let i = 0; i < LAND_MASK_SECTORS; i += 1) {
    if (radii[i] > 0) continue;
    let prev = (i + LAND_MASK_SECTORS - 1) % LAND_MASK_SECTORS;
    while (radii[prev] === 0) prev = (prev + LAND_MASK_SECTORS - 1) % LAND_MASK_SECTORS;
    let next = (i + 1) % LAND_MASK_SECTORS;
    while (radii[next] === 0) next = (next + 1) % LAND_MASK_SECTORS;
    radii[i] = Math.max(radii[prev], radii[next]);
  }

  for (let pass = 0; pass < 2; pass += 1) {
    const prev = [...radii];
    for (let i = 0; i < LAND_MASK_SECTORS; i += 1) {
      const left = prev[(i + LAND_MASK_SECTORS - 1) % LAND_MASK_SECTORS];
      const right = prev[(i + 1) % LAND_MASK_SECTORS];
      radii[i] = Math.max(originalRadii[i], (left + 2 * prev[i] + right) / 4);
    }
  }

  const buffer = Math.max(LAND_MASK_BUFFER_CM, 30 * MAP_PIXEL_TO_CM);
  const angularMargin = 1 / Math.cos(Math.PI / LAND_MASK_SECTORS);
  return radii.map((radius, i) => {
    const angle = -Math.PI + ((i + 0.5) * 2 * Math.PI) / LAND_MASK_SECTORS;
    const outRadius = radius * angularMargin + buffer;
    return [center.x + Math.cos(angle) * outRadius, center.y + Math.sin(angle) * outRadius];
  });
}

// Distance to nearest water body helper.
// NOTE: We only use static water body rectangles and water wells.
// The old "coast edge" checks (x < -250000 → dist=0) were REMOVED because the map's
// western/northern edges are mountain walls, not ocean — those checks caused the optimizer
// to treat map border areas as having free water access, pulling results to the edge.

// Call the Rust API server or fall back to JS optimizer
// Call the Rust API server
async function runGlobalOptimization() {
  const { rawNodes, config } = state;
  if (!rawNodes.length) return;

  // Show loading state
  els.mapLoading.innerHTML = `<span style="font-weight: 700; letter-spacing: 1px;">COMPUTING OPTIMAL SITES...</span>
    <span style="font-size: 0.8rem; color: var(--color-text-muted); margin-top: 6px;">Calling Rust optimizer...</span>`;
  els.mapLoading.classList.add("active");

  try {
    const weights = nonZeroWeights(config.weights);
    if (!hasOptimizationObjective(config.weights)) {
      els.mapLoading.innerHTML = `<span style="color: #ff3333; font-weight: bold; font-size: 1.1rem; margin-bottom: 12px;">OPTIMIZATION FAILED: Select at least one weighted resource.</span>
        <span style="font-size: 0.8rem; color: var(--color-text-muted);">Enable a resource slider or apply a phase preset, then try again.</span>`;
      els.mapLoading.classList.add("active");
      return;
    }

    // Build the request body
    const reqBody = {
      utility_func: config.utilityFunc,
      decay_func: config.decayFunc,
      purity_override: config.purityOverride,
      strategy: config.strategy,
      game_phase: config.gamePhase,
      sigma: config.sigma,
      ignore_spawns: config.ignoreSpawns,
      weights,
    };

    const apiRes = await fetch("/api/optimize", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(reqBody),
      signal: AbortSignal.timeout(30000),
    });

    if (!apiRes.ok) throw new Error(`API error ${apiRes.status}`);

    const results = await apiRes.json();

    // The Rust API returns local_nodes as a flat object, which JS can use directly
    state.results = results.slice(0, 3);
    state.selectedResultIdx = 0;

    renderMapOverlay();
    renderResultsPanel();
    els.mapLoading.classList.remove("active");
  } catch (apiErr) {
    console.error("Rust API error:", apiErr.message);
    els.mapLoading.innerHTML = `<span style="color: #ff3333; font-weight: bold; font-size: 1.1rem; margin-bottom: 12px;">OPTIMIZATION FAILED: ${apiErr.message}</span>
      <span style="font-size: 0.8rem; color: var(--color-text-muted);">Please make sure the Rust API server is running on port 8080.</span>
      <button class="btn btn-compute" style="margin-top: 12px; width: auto; padding: 8px 16px;" onclick="location.reload()">Reset View</button>`;
    els.mapLoading.classList.add("active");
  }
}

// Heatmap rendering has been deprecated and removed.

// Render SVG markers layer (nodes, spawns, target crosshairs)
function renderMapOverlay() {
  const svg = els.svgOverlay;
  svg.innerHTML = ""; // Clear existing markers

  // Set Viewbox coordinates to 1280x1280 so rendering is completely responsive
  svg.setAttribute("viewBox", "0 0 1280 1280");

  const activeKeys = Object.keys(state.config.weights).filter(
    (k) => Math.abs(state.config.weights[k]) > 0,
  );
  const selectedResult = state.results[state.selectedResultIdx];
  const inspectionRows = selectedResult
    ? nearbyWeightedNodes(
        state.rawNodes,
        selectedResult,
        state.config.weights,
        state.config.sigma,
        Number.POSITIVE_INFINITY,
      )
    : [];
  const inspectedNodeKeys = new Set(inspectionRows.map((row) => row.key));

  // 1. Draw Resource Nodes (if layer visible)
  if (state.visibleLayers.nodes && state.rawNodes.length > 0) {
    state.rawNodes.forEach((node) => {
      // Only render nodes that are currently weighted
      if (!activeKeys.includes(node.resource_type)) return;

      const res = RESOURCES.find((r) => r.id === node.resource_type);
      if (!res) return;

      const pix = gameToPixel(node.x, node.y);

      const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
      circle.setAttribute("cx", pix.x);
      circle.setAttribute("cy", pix.y);
      circle.setAttribute("r", "3.5");
      circle.setAttribute("fill", res.color);
      circle.setAttribute("stroke", "#06090e");
      circle.setAttribute("stroke-width", "0.5");
      const classes = ["node-marker"];
      if (selectedResult) {
        if (inspectedNodeKeys.has(nodeInspectionKey(node))) {
          classes.push("inspected-node");
        } else {
          classes.push("dimmed-node");
        }
      }
      circle.setAttribute("class", classes.join(" "));

      // Node description tooltip content
      const purityStr =
        node.purityMultiplier > 1.5 ? "Pure" : node.purityMultiplier < 0.8 ? "Impure" : "Normal";

      // Build appropriate label based on resource category
      const resObj = RESOURCES.find((r) => r.id === node.resource_type);
      let secondaryLabel;
      if (resObj) {
        if (resObj.category === "threat") {
          secondaryLabel = `<div class="purity" style="color: #ff6666;">⚠ Environmental Hazard</div>`;
        } else if (resObj.category === "collectible") {
          secondaryLabel = `<div class="purity" style="color: ${resObj.color};">✦ Collectible</div>`;
        } else {
          secondaryLabel = `<div class="purity" style="color: ${resObj.color}">${purityStr} Yield</div>`;
        }
      } else {
        secondaryLabel = `<div class="purity">${purityStr} Yield</div>`;
      }

      const tooltipText = `
        <div class="title">${res.name}</div>
        ${secondaryLabel}
        ${node.obstructed ? '<div style="color: #ff3333; font-weight: bold; font-size: 0.7rem; margin-top: 4px;">OBSTRUCTED (Requires Nobelisks)</div>' : ""}
        <div class="coordinates">X: ${Math.round(node.x).toLocaleString()}, Y: ${Math.round(node.y).toLocaleString()}</div>
      `;

      circle.addEventListener("mouseover", (e) => showTooltip(e, tooltipText));
      circle.addEventListener("mouseout", hideTooltip);

      svg.appendChild(circle);
    });
  }

  if (buildableLandPolygon.length > 2) {
    // Draw practical buildable land polygon used by the optimizer.
    const borderPoints = buildableLandPolygon
      .map((pt) => {
        const pix = gameToPixel(pt[0], pt[1]);
        return `${pix.x},${pix.y}`;
      })
      .join(" ");

    const borderPoly = document.createElementNS("http://www.w3.org/2000/svg", "polygon");
    borderPoly.setAttribute("points", borderPoints);
    borderPoly.setAttribute("fill", "rgba(0, 255, 170, 0.04)");
    borderPoly.setAttribute("stroke", "rgba(0, 255, 170, 0.55)");
    borderPoly.setAttribute("stroke-width", "2.0");
    borderPoly.setAttribute("stroke-dasharray", "6 4");
    borderPoly.style.pointerEvents = "none";
    svg.appendChild(borderPoly);
  }

  if (selectedResult) {
    const pix = gameToPixel(selectedResult.x, selectedResult.y);
    const radiusCircle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    radiusCircle.setAttribute("cx", pix.x);
    radiusCircle.setAttribute("cy", pix.y);
    radiusCircle.setAttribute("r", state.config.sigma * 0.13653321);
    radiusCircle.setAttribute("class", "inspection-radius");
    radiusCircle.style.pointerEvents = "none";
    svg.appendChild(radiusCircle);
  }

  // 2. Draw Start Spawn Pod locations (S) and starting area circles
  const ignoreSpawns = state.config.ignoreSpawns;

  DEFAULT_SPAWNS.forEach((spawn) => {
    const pix = gameToPixel(spawn.x, spawn.y);

    // Starting area radius is static and represents where the player spawns
    const boundaryRadiusPix = spawn.radius * 0.13653321;

    const boundaryCircle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    boundaryCircle.setAttribute("cx", pix.x);
    boundaryCircle.setAttribute("cy", pix.y);
    boundaryCircle.setAttribute("r", boundaryRadiusPix);
    boundaryCircle.setAttribute(
      "fill",
      ignoreSpawns ? "rgba(255, 152, 0, 0.01)" : "rgba(255, 152, 0, 0.06)",
    );
    boundaryCircle.setAttribute(
      "stroke",
      ignoreSpawns ? "rgba(255, 152, 0, 0.15)" : "rgba(255, 152, 0, 0.45)",
    );
    boundaryCircle.setAttribute("stroke-width", ignoreSpawns ? "1.0" : "2.0");
    if (ignoreSpawns) {
      boundaryCircle.setAttribute("stroke-dasharray", "4 4");
    }
    boundaryCircle.style.pointerEvents = "none";
    svg.appendChild(boundaryCircle);

    // Circle base
    const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    circle.setAttribute("cx", pix.x);
    circle.setAttribute("cy", pix.y);
    circle.setAttribute("r", "12");
    circle.setAttribute("fill", "#1b202c");
    circle.setAttribute("stroke", "#ff9800");
    circle.setAttribute("stroke-width", "2");
    circle.setAttribute("class", "spawn-marker");

    // Label letter 'S'
    const text = document.createElementNS("http://www.w3.org/2000/svg", "text");
    text.setAttribute("x", pix.x);
    text.setAttribute("y", pix.y + 4.5);
    text.setAttribute("font-family", "Outfit");
    text.setAttribute("font-size", "13");
    text.setAttribute("font-weight", "800");
    text.setAttribute("fill", "#ffffff");
    text.setAttribute("text-anchor", "middle");
    text.style.pointerEvents = "none";

    text.textContent = "S";

    const tooltipText = `
      <div class="title">Spawn Area: ${spawn.name}</div>
      <div style="font-size: 0.75rem; margin-bottom: 4px;">${spawn.description}</div>
      <div class="coordinates">X: ${spawn.x.toLocaleString()}, Y: ${spawn.y.toLocaleString()}</div>
    `;

    circle.addEventListener("mouseover", (e) => showTooltip(e, tooltipText));
    circle.addEventListener("mouseout", hideTooltip);

    svg.appendChild(circle);
    svg.appendChild(text);
  });

  // 3. Draw Optimal Results crosshairs (Top 3)
  state.results.forEach((res, idx) => {
    const pix = gameToPixel(res.x, res.y);
    const isSelected = idx === state.selectedResultIdx;

    const color = isSelected ? "#ff9800" : "#e0a900";
    const radius = isSelected ? 18 : 13;
    const strokeWidth = isSelected ? 2.5 : 1.5;
    const pulseClass = isSelected ? "pulsing-site" : "";

    // Group wrapper
    const g = document.createElementNS("http://www.w3.org/2000/svg", "g");
    g.setAttribute("class", `site-marker ${pulseClass}`);

    // Outer dash circle
    const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    circle.setAttribute("cx", pix.x);
    circle.setAttribute("cy", pix.y);
    circle.setAttribute("r", radius);
    circle.setAttribute("fill", "none");
    circle.setAttribute("stroke", color);
    circle.setAttribute("stroke-width", strokeWidth);
    circle.setAttribute("stroke-dasharray", "4 2");

    // Center point
    const center = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    center.setAttribute("cx", pix.x);
    center.setAttribute("cy", pix.y);
    center.setAttribute("r", "3");
    center.setAttribute("fill", color);

    // Line ticks
    const gap = isSelected ? 6 : 4;

    const lineL = document.createElementNS("http://www.w3.org/2000/svg", "line");
    lineL.setAttribute("x1", pix.x - radius - gap);
    lineL.setAttribute("y1", pix.y);
    lineL.setAttribute("x2", pix.x - gap);
    lineL.setAttribute("y2", pix.y);
    lineL.setAttribute("stroke", color);
    lineL.setAttribute("stroke-width", strokeWidth);

    const lineR = document.createElementNS("http://www.w3.org/2000/svg", "line");
    lineR.setAttribute("x1", pix.x + gap);
    lineR.setAttribute("y1", pix.y);
    lineR.setAttribute("x2", pix.x + radius + gap);
    lineR.setAttribute("y2", pix.y);
    lineR.setAttribute("stroke", color);
    lineR.setAttribute("stroke-width", strokeWidth);

    const lineT = document.createElementNS("http://www.w3.org/2000/svg", "line");
    lineT.setAttribute("x1", pix.x);
    lineT.setAttribute("y1", pix.y - radius - gap);
    lineT.setAttribute("x2", pix.x);
    lineT.setAttribute("y2", pix.y - gap);
    lineT.setAttribute("stroke", color);
    lineT.setAttribute("stroke-width", strokeWidth);

    const lineB = document.createElementNS("http://www.w3.org/2000/svg", "line");
    lineB.setAttribute("x1", pix.x);
    lineB.setAttribute("y1", pix.y + gap);
    lineB.setAttribute("x2", pix.x);
    lineB.setAttribute("y2", pix.y + radius + gap);
    lineB.setAttribute("stroke", color);
    lineB.setAttribute("stroke-width", strokeWidth);

    // Text label rank
    const label = document.createElementNS("http://www.w3.org/2000/svg", "text");
    label.setAttribute("x", pix.x);
    label.setAttribute("y", pix.y - radius - gap - 6);
    label.setAttribute("font-family", "Outfit");
    label.setAttribute("font-size", isSelected ? "12" : "10");
    label.setAttribute("font-weight", "800");
    label.setAttribute("fill", color);
    label.setAttribute("text-anchor", "middle");
    label.style.textShadow = "0 2px 4px rgba(0,0,0,0.8)";
    label.textContent = `#${idx + 1} SITE`;

    g.appendChild(circle);
    g.appendChild(center);
    g.appendChild(lineL);
    g.appendChild(lineR);
    g.appendChild(lineT);
    g.appendChild(lineB);
    g.appendChild(label);

    // Add interactivity to the group
    g.style.pointerEvents = "auto";
    g.addEventListener("click", () => {
      selectResult(idx);
    });

    svg.appendChild(g);
  });
}

// Render floating results list
function renderResultsPanel() {
  els.resultsList.innerHTML = "";

  if (state.results.length === 0) {
    els.resultsList.innerHTML =
      '<div style="padding: 24px; color: var(--color-text-muted); text-align: center; font-size: 0.85rem; font-family: var(--font-display); letter-spacing: 1px;">CLICK "COMPUTE STARTING AREAS" TO RUN SIMULATION</div>';
    return;
  }

  state.results.forEach((res, idx) => {
    const isSelected = idx === state.selectedResultIdx;

    const card = document.createElement("div");
    card.className = `result-card ${isSelected ? "active" : ""}`;

    // Compute local node summary
    const nodeItems = [];
    for (const label in res.local_nodes) {
      nodeItems.push(`${res.local_nodes[label]}x ${label}`);
    }
    const nodesSummary = nodeItems.slice(0, 4).join(", ") + (nodeItems.length > 4 ? "..." : "");
    const yields = topResourceYields(res.resource_yields || {}, 4);
    const ruggedness = Number.isFinite(res.terrain_ruggedness) ? res.terrain_ruggedness : 0;
    const diversity = Number.isFinite(res.diversity_score) ? res.diversity_score : 0;
    const obstructedCount = countObstructedNodes(res.obstructed_nodes);

    card.innerHTML = `
      <div class="result-card-title">
        <span class="rank-badge">#${idx + 1} OPTIMAL BASE</span>
        <span class="score-val">${res.score.toFixed(4)}</span>
      </div>
      <div class="result-card-coords">
        X: ${Math.round(res.x).toLocaleString()}, Y: ${Math.round(res.y).toLocaleString()} (${res.closest_spawn.name})
      </div>
      <div class="result-card-details">
        <strong>Nodes in range (${state.config.sigma}m):</strong> ${nodesSummary || "None"}
      </div>
      <div class="result-metrics">
        <div class="result-metric">
          <span>Terrain</span>
          <strong>${formatRuggedness(ruggedness)}</strong>
        </div>
        <div class="result-metric">
          <span>Diversity</span>
          <strong>${formatDiversity(diversity)}</strong>
        </div>
      </div>
    `;

    const yieldStrip = document.createElement("div");
    yieldStrip.className = "yield-strip";
    const yieldLabel = document.createElement("span");
    yieldLabel.className = "yield-label";
    yieldLabel.textContent = "Top yields";
    yieldStrip.appendChild(yieldLabel);

    if (yields.length > 0) {
      yields.forEach((resourceYield) => {
        const pill = document.createElement("span");
        pill.className = "yield-pill";

        const dot = document.createElement("span");
        dot.className = "yield-dot";
        dot.style.backgroundColor = resourceYield.color;
        pill.appendChild(dot);

        const name = document.createElement("span");
        name.textContent = resourceYield.name;
        pill.appendChild(name);

        const value = document.createElement("strong");
        value.textContent = formatYield(resourceYield.value);
        pill.appendChild(value);

        yieldStrip.appendChild(pill);
      });
    } else {
      const empty = document.createElement("span");
      empty.className = "yield-empty";
      empty.textContent = "None";
      yieldStrip.appendChild(empty);
    }

    card.appendChild(yieldStrip);

    if (isSelected) {
      const inspectionRows = nearbyWeightedNodes(
        state.rawNodes,
        res,
        state.config.weights,
        state.config.sigma,
        8,
      );
      const inspectionList = document.createElement("div");
      inspectionList.className = "inspection-list";

      const inspectionLabel = document.createElement("span");
      inspectionLabel.className = "yield-label";
      inspectionLabel.textContent = "Nearby contributors";
      inspectionList.appendChild(inspectionLabel);

      if (inspectionRows.length > 0) {
        inspectionRows.forEach((row) => {
          const item = document.createElement("div");
          item.className = "inspection-row";

          const dot = document.createElement("span");
          dot.className = "yield-dot";
          dot.style.backgroundColor = row.color;
          item.appendChild(dot);

          const body = document.createElement("div");
          body.className = "inspection-row-body";

          const title = document.createElement("span");
          title.className = "inspection-row-title";
          title.textContent = row.name;
          body.appendChild(title);

          const meta = document.createElement("span");
          meta.className = "inspection-row-meta";
          meta.textContent = `${row.purity} - ${Math.round(row.distance)}m`;
          body.appendChild(meta);

          item.appendChild(body);

          const contribution = document.createElement("strong");
          contribution.className = "inspection-contribution";
          contribution.textContent = formatContribution(row.contribution);
          item.appendChild(contribution);

          if (row.obstructed) {
            const badge = document.createElement("span");
            badge.className = "inspection-badge";
            badge.textContent = "Obstructed";
            item.appendChild(badge);
          }

          inspectionList.appendChild(item);
        });
      } else {
        const empty = document.createElement("span");
        empty.className = "yield-empty";
        empty.textContent = "None";
        inspectionList.appendChild(empty);
      }

      card.appendChild(inspectionList);
    }

    if (obstructedCount > 0) {
      const obstructedNote = document.createElement("div");
      obstructedNote.className = "obstructed-note";
      obstructedNote.textContent = `${obstructedCount} obstructed node${obstructedCount === 1 ? "" : "s"} excluded`;
      card.appendChild(obstructedNote);
    }

    card.addEventListener("click", () => {
      selectResult(idx);
    });

    els.resultsList.appendChild(card);
  });
}

// Select a specific site rank card
function selectResult(idx) {
  state.selectedResultIdx = idx;
  renderResultsPanel();
  renderMapOverlay();
}

// Tooltip helpers
function showTooltip(e, html) {
  const tooltip = els.mapTooltip;
  tooltip.innerHTML = html;
  tooltip.style.display = "block";

  // Position relative to screen
  const x = e.clientX + 15;
  const y = e.clientY + 15;

  tooltip.style.left = `${x}px`;
  tooltip.style.top = `${y}px`;
}

function hideTooltip() {
  els.mapTooltip.style.display = "none";
}

// Dynamic weight sliders renderer
function renderWeightSliders() {
  els.weightsContainer.innerHTML = "";

  const groups = {
    core: { title: "Production Resources", container: document.createElement("div") },
    collectible: { title: "Collectibles & Research", container: document.createElement("div") },
    threat: { title: "Environmental Threats", container: document.createElement("div") },
  };

  for (const key in groups) {
    const section = document.createElement("div");
    section.style.marginBottom = "16px";
    const title = document.createElement("h3");
    title.style.fontSize = "0.75rem";
    title.style.color = "var(--color-text-muted)";
    title.style.marginBottom = "8px";
    title.style.textTransform = "uppercase";
    title.style.letterSpacing = "0.5px";
    title.textContent = groups[key].title;

    section.appendChild(title);
    groups[key].container.className = "weights-list";
    section.appendChild(groups[key].container);
    els.weightsContainer.appendChild(section);
  }

  RESOURCES.forEach((res) => {
    const val = state.config.weights[res.id] !== undefined ? state.config.weights[res.id] : 0.0;
    const isExcluded = val === 0.0;

    const row = document.createElement("div");
    row.className = "weight-row";

    row.innerHTML = `
      <div class="weight-label" style="display: flex; align-items: center; gap: 8px;">
        <input type="checkbox" class="weight-toggle" id="toggle-${res.id}" ${!isExcluded ? "checked" : ""} style="cursor: pointer;">
        <span class="weight-indicator" style="background-color: ${res.color}"></span>
        <span class="weight-name" style="${isExcluded ? "color: var(--color-text-dark); text-decoration: line-through;" : ""}">${res.name}</span>
      </div>
      <div class="weight-slider-container">
        <input type="range" min="-2.00" max="2.00" step="0.01" value="${val.toFixed(2)}" id="slider-${res.id}" ${isExcluded ? "disabled" : ""}>
        <span class="weight-val" id="val-${res.id}" style="${isExcluded ? "color: var(--color-text-dark);" : ""}">${val.toFixed(2)}</span>
      </div>
    `;

    const checkbox = row.querySelector(".weight-toggle");
    const slider = row.querySelector("input[type='range']");
    const valSpan = row.querySelector(".weight-val");
    const nameSpan = row.querySelector(".weight-name");

    checkbox.addEventListener("change", (e) => {
      const active = e.target.checked;
      if (active) {
        slider.removeAttribute("disabled");
        nameSpan.style.color = "";
        nameSpan.style.textDecoration = "";
        valSpan.style.color = "";

        let v = parseFloat(slider.value);
        if (v === 0.0) {
          if (res.category === "threat") {
            v = -1.0;
          } else if (res.category === "collectible") {
            v = 0.05;
          } else {
            v = 1.0;
          }
          slider.value = v.toFixed(2);
        }
        state.config.weights[res.id] = v;
        valSpan.textContent = v.toFixed(2);
      } else {
        slider.setAttribute("disabled", "true");
        nameSpan.style.color = "var(--color-text-dark)";
        nameSpan.style.textDecoration = "line-through";
        valSpan.style.color = "var(--color-text-dark)";

        state.config.weights[res.id] = 0.0;
        valSpan.textContent = "0.00";
      }
      clearComputation();
    });

    slider.addEventListener("input", (e) => {
      const v = parseFloat(e.target.value);
      state.config.weights[res.id] = v;
      valSpan.textContent = v.toFixed(2);
      clearComputation();
    });

    groups[res.category].container.appendChild(row);
  });
}

// Clear previous computation overlays
function clearComputation() {
  state.results = [];
  renderMapOverlay();
  renderResultsPanel();
}

function setWalkingRadius(value) {
  const sigma = Math.max(50, Math.min(1000, Math.round(Number(value) / 50) * 50));
  state.config.sigma = sigma;
  els.paramSigma.value = String(sigma);
  els.paramSigmaValue.textContent = `${sigma}m`;
}

// Preset loader for Game Phase
function applyPhasePreset(phaseId) {
  state.config.gamePhase = phaseId;

  // Reset all weights to 0.0
  RESOURCES.forEach((res) => {
    state.config.weights[res.id] = 0.0;
  });

  // Apply preset values
  const preset = PRESETS[phaseId];
  if (preset) {
    for (const k in preset) {
      state.config.weights[k] = preset[k];
    }
  }

  // Apply preset spawn behavior and walking radius dynamically.
  const rawPreset = PRESETS_RAW.find((p) => p.id === phaseId);
  if (rawPreset) {
    state.config.ignoreSpawns = rawPreset.ignore_spawns;
    els.paramIgnoreSpawns.value = rawPreset.ignore_spawns ? "true" : "false";
    setWalkingRadius(rawPreset.sigma);
  } else {
    // Fallbacks just in case
    state.config.ignoreSpawns = false;
    els.paramIgnoreSpawns.value = "false";
    setWalkingRadius(200);
  }

  // Redraw weight sliders UI
  renderWeightSliders();

  // Clear previous calculation to prompt manual recalculation
  clearComputation();
}

// Setup Event Listeners
function setupEvents() {
  // Preset Phase Buttons
  document.querySelectorAll(".preset-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".preset-btn").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      applyPhasePreset(btn.dataset.phase);
    });
  });

  // Map Type Toggle Buttons
  document.querySelectorAll(".map-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".map-btn").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");

      const type = btn.dataset.map;
      state.mapType = type;
      if (type === "game") {
        els.mapImg.src = "/src/assets/game_map.png";
      } else {
        els.mapImg.src = "/src/assets/realistic_map.png";
      }
    });
  });

  // Parameter Inputs
  els.paramUtility.addEventListener("change", (e) => {
    state.config.utilityFunc = e.target.value;
    clearComputation();
  });

  els.paramDecay.addEventListener("change", (e) => {
    state.config.decayFunc = e.target.value;
    clearComputation();
  });

  els.paramPurity.addEventListener("change", (e) => {
    state.config.purityOverride = e.target.value;
    clearComputation();
  });

  els.paramStrategy.addEventListener("change", (e) => {
    state.config.strategy = e.target.value;
    clearComputation();
  });

  const onSigmaInput = (e) => {
    setWalkingRadius(e.target.value);
    clearComputation();
  };
  els.paramSigma.addEventListener("input", onSigmaInput);
  els.paramSigma.addEventListener("change", onSigmaInput);

  els.paramIgnoreSpawns.addEventListener("change", (e) => {
    state.config.ignoreSpawns = e.target.value === "true";
    clearComputation();
  });

  els.btnCompute.addEventListener("click", () => {
    void runGlobalOptimization();
  });

  // Layer Toggles removed. Nodes are always rendered.

  // Document mousemove to drag/reposition tooltip smoothly
  document.addEventListener("mousemove", (e) => {
    if (els.mapTooltip.style.display === "block") {
      els.mapTooltip.style.left = `${e.clientX + 15}px`;
      els.mapTooltip.style.top = `${e.clientY + 15}px`;
    }
  });
}

// Setup interactive Zoom and Pan on the map container
function setupZoomAndPan() {
  const container = els.mapInnerContainer;
  const target = els.zoomContainer;

  let scale = 1.0;
  let panX = 0;
  let panY = 0;

  let isDragging = false;
  let startX = 0;
  let startY = 0;

  function applyTransform() {
    target.style.transform = `translate(${panX}px, ${panY}px) scale(${scale})`;
  }

  // Mouse Wheel Zoom
  container.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();

      const rect = container.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;

      // Position of cursor in target's coordinate space before zoom
      const localX = (mx - panX) / scale;
      const localY = (my - panY) / scale;

      const delta = -e.deltaY;
      const zoomFactor = delta > 0 ? 1.15 : 1 / 1.15;

      let newScale = scale * zoomFactor;
      newScale = Math.max(1.0, Math.min(20.0, newScale));

      if (newScale === 1.0) {
        panX = 0;
        panY = 0;
        scale = 1.0;
      } else {
        scale = newScale;
        panX = mx - localX * scale;
        panY = my - localY * scale;

        // Limit panning boundaries so map doesn't drift completely out of view
        const minPanX = rect.width * (1 - scale);
        const minPanY = rect.height * (1 - scale);
        panX = Math.min(0, Math.max(minPanX, panX));
        panY = Math.min(0, Math.max(minPanY, panY));
      }

      applyTransform();
    },
    { passive: false },
  );

  // Drag to Pan
  container.addEventListener("mousedown", (e) => {
    // Only drag with left click
    if (e.button !== 0) return;

    isDragging = true;
    container.style.cursor = "grabbing";
    startX = e.clientX - panX;
    startY = e.clientY - panY;

    e.preventDefault(); // Prevent text/image selection
  });

  document.addEventListener("mousemove", (e) => {
    if (!isDragging) return;

    panX = e.clientX - startX;
    panY = e.clientY - startY;

    const rect = container.getBoundingClientRect();
    const minPanX = rect.width * (1 - scale);
    const minPanY = rect.height * (1 - scale);
    panX = Math.min(0, Math.max(minPanX, panX));
    panY = Math.min(0, Math.max(minPanY, panY));

    applyTransform();
  });

  const endDrag = () => {
    if (!isDragging) return;
    isDragging = false;
    container.style.cursor = "grab";
  };

  document.addEventListener("mouseup", endDrag);
  container.addEventListener("mouseleave", endDrag);

  // Initially set grab cursor style
  container.style.cursor = "grab";
}

// Initializer
async function init() {
  setupEvents();
  setupZoomAndPan();

  // Show initial loading state
  els.mapLoading.innerHTML = `<span style="font-weight: 700; letter-spacing: 1px;">LOADING MAP DATA...</span>`;
  els.mapLoading.classList.add("active");

  try {
    // 1. Fetch presets from backend API
    const presetsRes = await fetch("/api/presets");
    if (!presetsRes.ok) throw new Error("Unable to contact backend FICSIT API server for presets.");
    const presetsData = await presetsRes.json();
    PRESETS_RAW = presetsData;
    presetsData.forEach((p) => {
      PRESETS[p.id] = p.weights;
    });

    // 2. Fetch spawns from backend API
    const spawnsRes = await fetch("/api/spawns");
    if (!spawnsRes.ok) throw new Error("Unable to contact backend FICSIT API server for spawns.");
    DEFAULT_SPAWNS = await spawnsRes.json();

    // 3. Populate default phase preset
    applyPhasePreset("phase1");

    // 4. Fetch normalized map nodes from the backend parser
    const nodesRes = await fetch("/api/nodes");
    if (!nodesRes.ok) throw new Error("Unable to contact backend FICSIT API server for nodes.");

    state.rawNodes = await nodesRes.json();
    buildableLandPolygon = computeBuildableLandPolygon(state.rawNodes);

    console.log(`Database loaded: ${state.rawNodes.length} nodes parsed successfully.`);

    // Draw initial resource nodes and biomes, leaving results cleared until computed
    clearComputation();
    els.mapLoading.classList.remove("active");
  } catch (err) {
    console.error("Failed to initialize: ", err);
    els.mapLoading.innerHTML = `<span style="color: #ff3333; font-weight: bold; font-size: 1.1rem; margin-bottom: 12px;">INITIALIZATION FAILED</span>
      <span style="font-size: 0.8rem; color: var(--color-text-muted); text-align: center; max-width: 80%;">${err.message}<br><br>Please ensure the Rust backend server is running on port 8080 and reload the page.</span>`;
    els.mapLoading.classList.add("active");
  }
}

// Run app init
void init();
