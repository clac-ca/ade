import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { runCommand, runCommandCapture } from "./shared.mjs";

const defaultGitUserName = "github-actions[bot]";
const defaultGitUserEmail =
  "41898282+github-actions[bot]@users.noreply.github.com";

async function fetchTags(cwd) {
  await runCommand("git", ["fetch", "--tags", "origin"], {
    cwd,
  });
}

async function tagExists(tag, cwd) {
  try {
    await runCommandCapture(
      "git",
      ["rev-parse", "-q", "--verify", `refs/tags/${tag}`],
      {
        cwd,
      },
    );
    return true;
  } catch {
    return false;
  }
}

async function readTagContents(tag, cwd) {
  const { stdout } = await runCommandCapture(
    "git",
    ["for-each-ref", `refs/tags/${tag}`, "--format=%(contents)"],
    {
      cwd,
    },
  );

  const contents = stdout.trim();

  if (contents === "") {
    throw new Error(`Tag ${tag} does not contain metadata.`);
  }

  return contents;
}

async function readJsonTag(tag, cwd) {
  return JSON.parse(await readTagContents(tag, cwd));
}

async function listTags(pattern, cwd) {
  const { stdout } = await runCommandCapture(
    "git",
    ["tag", "--list", pattern, "--sort=-refname"],
    {
      cwd,
    },
  );

  return stdout
    .split("\n")
    .map((tag) => tag.trim())
    .filter(Boolean);
}

async function createJsonTag({ cwd, payload, tag, targetRef }) {
  const tempDir = mkdtempSync(join(tmpdir(), "ade-tag-"));
  const messagePath = join(tempDir, "message.json");

  try {
    writeFileSync(messagePath, JSON.stringify(payload, null, 2) + "\n");
    await runCommand("git", ["config", "user.name", defaultGitUserName], {
      cwd,
    });
    await runCommand("git", ["config", "user.email", defaultGitUserEmail], {
      cwd,
    });
    await runCommand("git", ["tag", "-a", tag, targetRef, "-F", messagePath], {
      cwd,
    });
    await runCommand("git", ["push", "origin", tag], {
      cwd,
    });
  } finally {
    rmSync(tempDir, {
      force: true,
      recursive: true,
    });
  }
}

export { createJsonTag, fetchTags, listTags, readJsonTag, tagExists };
