// Bumps the app version everywhere it is written, so About, the installers
// and the updater always agree.
//
//   pnpm bump            0.1.0 -> 0.1.1  (patch)
//   pnpm bump minor      0.1.0 -> 0.2.0
//   pnpm bump major      0.1.0 -> 1.0.0
//   pnpm bump 1.2.3      set an exact version
//   pnpm bump --check    verify all files agree (and match $TAG_VERSION if set)
//
// Files: package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and
// the app's entry in src-tauri/Cargo.lock.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const files = {
  pkg: join(root, "package.json"),
  conf: join(root, "src-tauri", "tauri.conf.json"),
  cargo: join(root, "src-tauri", "Cargo.toml"),
  lock: join(root, "src-tauri", "Cargo.lock"),
};
const CRATE = "yet-another-pdf-editor";
const SEMVER = /^\d+\.\d+\.\d+$/;

const read = (f) => readFileSync(f, "utf8");
const cargoVersion = (text) => text.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
const lockVersion = (text) => text.match(new RegExp(`name = "${CRATE}"\\r?\\nversion = "([^"]+)"`))?.[1];

function current() {
  return {
    "package.json": JSON.parse(read(files.pkg)).version,
    "tauri.conf.json": JSON.parse(read(files.conf)).version,
    "Cargo.toml": cargoVersion(read(files.cargo)),
    "Cargo.lock": lockVersion(read(files.lock)),
  };
}

const arg = process.argv[2] ?? "patch";

if (arg === "--check") {
  const v = current();
  const distinct = new Set(Object.values(v));
  const tag = process.env.TAG_VERSION?.replace(/^v/, "");
  if (distinct.size !== 1) {
    console.error("Version mismatch:", v);
    process.exit(1);
  }
  const [version] = distinct;
  if (tag && tag !== version) {
    console.error(`Tag v${tag} does not match the app version ${version}. Run "pnpm bump ${tag}" and commit first.`);
    process.exit(1);
  }
  console.log(`Version ${version} is consistent${tag ? " and matches the tag" : ""}.`);
  process.exit(0);
}

const from = JSON.parse(read(files.conf)).version;
let next;
if (SEMVER.test(arg)) {
  next = arg;
} else {
  const [ma, mi, pa] = from.split(".").map(Number);
  next =
    arg === "major" ? `${ma + 1}.0.0` : arg === "minor" ? `${ma}.${mi + 1}.0` : arg === "patch" ? `${ma}.${mi}.${pa + 1}` : null;
}
if (!next) {
  console.error(`Unknown bump "${arg}". Use patch, minor, major or an exact version like 1.2.3.`);
  process.exit(1);
}

// JSON files: replace only the version value to keep formatting intact.
for (const f of [files.pkg, files.conf]) {
  const text = read(f);
  writeFileSync(f, text.replace(/("version"\s*:\s*")[^"]+(")/, `$1${next}$2`));
}
writeFileSync(files.cargo, read(files.cargo).replace(/^(\[package\][\s\S]*?^version\s*=\s*")[^"]+(")/m, `$1${next}$2`));
writeFileSync(
  files.lock,
  read(files.lock).replace(new RegExp(`(name = "${CRATE}"\\r?\\nversion = ")[^"]+(")`), `$1${next}$2`),
);

const after = current();
if (new Set(Object.values(after)).size !== 1 || after["tauri.conf.json"] !== next) {
  console.error("Bump failed, files disagree:", after);
  process.exit(1);
}
console.log(`Version ${from} -> ${next}`);
console.log(`Next: commit, then tag and push:  git tag v${next} && git push origin v${next}`);
