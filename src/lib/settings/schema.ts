/**
 * Walking `config.schema.json` down to its leaf key paths.
 *
 * "Leaf" means a key a single control edits. Three shapes stop the walk early
 * and are the reason this is not a three-line recursion:
 *
 *   * a union (`oneOf` / `anyOf`) — `accounts.*.token` is one of four table
 *     shapes and `watches.*.project` is a number or a string, so descending
 *     would produce paths no TOML file ever contains;
 *   * a dictionary table (`additionalProperties` holding a schema) — the KEYS
 *     are the user's, so the path gets one `*` segment. `watches.*.jobs.*` is
 *     one leaf because the whole ordered table is edited together;
 *   * an array — of scalars it is one leaf, of tables it is `*` and the walk
 *     continues, which is what turns `[[watches]]` into `watches.*`.
 */

export interface JsonSchema {
  $ref?: string;
  $defs?: Record<string, JsonSchema>;
  type?: string | string[];
  properties?: Record<string, JsonSchema>;
  additionalProperties?: JsonSchema | boolean;
  items?: JsonSchema;
  oneOf?: JsonSchema[];
  const?: unknown;
  anyOf?: JsonSchema[];
  enum?: unknown[];
  description?: string;
}

/** Resolve `$ref` chains against the root's `$defs`. */
function deref(node: JsonSchema, root: JsonSchema): JsonSchema {
  let current = node;
  // A bounded loop rather than recursion: a self-referential $ref in a
  // generated schema would otherwise hang `npm test` with no output.
  for (let i = 0; i < 32 && current.$ref; i++) {
    const name = current.$ref.split("/").pop() ?? "";
    const target = root.$defs?.[name];
    if (!target) return current;
    current = target;
  }
  return current;
}

/**
 * Every leaf key path in a config schema, sorted.
 *
 * @param root the parsed `config.schema.json`
 */
export function leafPaths(root: JsonSchema): string[] {
  const out: string[] = [];

  const walk = (node: JsonSchema, path: string, depth: number): void => {
    const here = deref(node, root);

    if (depth > 12) {
      out.push(path);
      return;
    }
    if (here.oneOf || here.anyOf || here.enum || here.const !== undefined) {
      out.push(path);
      return;
    }
    if (here.properties) {
      for (const [key, child] of Object.entries(here.properties)) {
        walk(child, path ? `${path}.${key}` : key, depth + 1);
      }
      return;
    }
    const extra = here.additionalProperties;
    if (extra && typeof extra === "object") {
      const sub = deref(extra, root);
      const wildcard = `${path}.*`;
      if (sub.properties || (sub.additionalProperties && typeof sub.additionalProperties === "object")) {
        walk(extra, wildcard, depth + 1);
      } else {
        out.push(wildcard);
      }
      return;
    }
    const type = Array.isArray(here.type) ? here.type[0] : here.type;
    if (type === "array" && here.items) {
      const item = deref(here.items, root);
      if (item.properties) {
        walk(here.items, `${path}.*`, depth + 1);
        return;
      }
      out.push(path);
      return;
    }
    out.push(path);
  };

  walk(root, "", 0);
  return out.sort();
}
