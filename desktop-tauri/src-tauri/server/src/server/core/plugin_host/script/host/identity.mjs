import { createHash } from "node:crypto"

const valid = id => typeof id === "string" && /^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$/.test(id)

export function allocateProvider(original, entry, occupied, claimed = {}) {
  if (!valid(original)) throw new Error("Invalid plugin provider id")
  const available = id => valid(id) && !id.startsWith("custom-") && !occupied.has(id)
    && (!Object.hasOwn(claimed, id) || claimed[id] === entry.id)
  const saved = entry.aliases?.[original]
  if (typeof saved === "string") {
    if (!available(saved)) throw new Error("Saved plugin provider identity is already in use")
    occupied.add(saved)
    return saved
  }
  let id = original
  if (!available(id)) {
    id = (original.startsWith("custom-") ? "magpie-" : "") + original.slice(0, 90) + "-plugin"
    if (!available(id)) id += "-" + createHash("sha256").update(entry.id).digest("hex").slice(0, 12)
  }
  if (!available(id)) throw new Error("Duplicated provider in plugin package")
  occupied.add(id)
  return id
}
