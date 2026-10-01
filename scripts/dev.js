#!/usr/bin/env bun

const isWindows = process.platform === "win32";

const children = [];
let shuttingDown = false;

function spawnProcess(label, command, args) {
  const child = Bun.spawn([command, ...args], {
    stdout: "inherit",
    stderr: "inherit",
    stdin: "inherit",
    env: process.env,
  });

  children.push({ label, child });

  void (async () => {
    const exitCode = await child.exited;
    if (!shuttingDown && exitCode !== 0) {
      console.error(`[dev] ${label} exited with code ${exitCode}. Shutting down...`);
      shutdown(exitCode || 1);
    }
  })();

  return child;
}

function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;

  for (const { child } of children) {
    try {
      child.kill();
    } catch {
      // Ignore already-exited child processes.
    }
  }

  setTimeout(() => process.exit(code), isWindows ? 500 : 100);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForServer(url, timeoutMs = 120_000, intervalMs = 500) {
  const start = Date.now();
  let printedWaiting = false;

  while (Date.now() - start < timeoutMs) {
    if (shuttingDown) return false;

    try {
      const response = await fetch(url);
      if (response.ok) return true;
    } catch {
      if (!printedWaiting) {
        console.log("[dev] Waiting for Rust API server to become ready...");
        printedWaiting = true;
      }
    }

    await sleep(intervalMs);
  }

  return false;
}

process.on("SIGINT", () => shutdown(0));
process.on("SIGTERM", () => shutdown(0));

console.log("[dev] Starting Rust API server on http://127.0.0.1:8080");
spawnProcess("server", "cargo", ["run", "--", "--server", "8080"]);

const serverReady = await waitForServer("http://127.0.0.1:8080/api/health");
if (!serverReady) {
  console.error("[dev] Rust API server did not become ready in time. Shutting down...");
  shutdown(1);
} else {
  // Small grace delay so Vite does not open the browser while the API is still settling.
  await sleep(500);
  console.log("[dev] Rust API server is ready.");
  console.log("[dev] Starting web UI on http://127.0.0.1:3000");
  spawnProcess("ui", "bun", ["x", "vp", "dev"]);
}

await new Promise(() => {});
