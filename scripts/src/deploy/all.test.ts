import assert from "node:assert/strict";
import { chmod, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { spawn } from "node:child_process";
import { test } from "node:test";

test("docker compose progress is visible while the command is still running", async () => {
  const binDir = await mkdtemp(join(tmpdir(), "solvent-compose-"));
  const docker = join(binDir, "docker");
  await writeFile(docker, "#!/bin/sh\nprintf 'compose progress visible\\n'\nsleep 5\n");
  await chmod(docker, 0o755);

  const child = spawn(process.execPath, ["scripts/src/deploy/all.ts", "--reset"], {
    cwd: process.cwd(),
    env: { ...process.env, PATH: `${binDir}${delimiter}${process.env.PATH ?? ""}` },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk: Buffer) => {
    stdout += chunk.toString();
  });
  child.stderr.on("data", (chunk: Buffer) => {
    stderr += chunk.toString();
  });

  try {
    await new Promise<void>((resolve, reject) => {
      const deadline = setTimeout(
        () => reject(new Error(`deploy reset did not start compose: ${stdout}`)),
        5_000,
      );
      const check = () => {
        if (stdout.includes("== infra: docker compose down -v ==")) {
          clearTimeout(deadline);
          resolve();
        } else {
          setTimeout(check, 10);
        }
      };
      check();
    });
    await new Promise((resolve) => setTimeout(resolve, 1_000));

    assert.equal(child.exitCode, null);
    assert.match(`${stdout}${stderr}`, /compose progress visible/);
  } finally {
    child.kill("SIGKILL");
  }
});
