import { readFile, realpath, stat } from "node:fs/promises"
import { resolve, relative, isAbsolute } from "node:path"
import { pathToFileURL } from "node:url"

const indexes = ["index.ts", "index.tsx", "index.js", "index.mjs", "index.cjs"]
const pick = value => typeof value === "string" ? value : value && typeof value === "object"
  ? pick(value.import) || pick(value.default) || pick(value.node) || pick(value.require) || pick(value.bun) : undefined

export async function manifestEntries(root, manifest) {
  const exports = manifest.exports
  const paths = [pick(exports?.["./server"]), manifest.main, pick(exports?.["."]), pick(exports), ...indexes]
  const found = []
  for (const entry of paths) {
    if (typeof entry !== "string" || !entry.trim()) continue
    const file = await realpath(resolve(root, entry)).catch(() => null)
    if (!file) continue
    const inside = relative(root, file)
    if (isAbsolute(inside) || inside === ".." || inside.startsWith("../") || inside.startsWith("..\\")) throw new Error("Plugin entry is outside its package")
    if ((await stat(file)).isFile() && !found.includes(file)) found.push(file)
  }
  if (!found.length) throw new Error("Package has no OpenCode provider entry")
  return found
}

export function servers(module) {
  const main = module.default
  if (main && typeof main === "object" && ("server" in main || "id" in main || "tui" in main)) {
    return typeof main.server === "function" ? [main.server] : []
  }
  return [...new Set(Object.values(module).map(value => typeof value === "function" ? value : value?.server)
    .filter(value => typeof value === "function"))]
}

export async function importProvider(root, manifest) {
  let failure
  for (const entry of await manifestEntries(root, manifest)) {
    try {
      const candidates = servers(await import(pathToFileURL(entry).href))
      if (candidates.length) return candidates
    } catch (error) { failure = error }
  }
  throw failure || new Error("Package exports no OpenCode v1 provider")
}

export async function bounded(work, milliseconds, description) {
  let timer
  try {
    return await Promise.race([Promise.resolve().then(work), new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${description} timed out`)), milliseconds)
    })])
  } finally { clearTimeout(timer) }
}

export async function readManifest(directory) {
  const root = await realpath(directory)
  return { root, manifest: JSON.parse(await readFile(resolve(root, "package.json"), "utf8")) }
}
