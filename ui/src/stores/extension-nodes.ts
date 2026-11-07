import type { Extension, NodeSettingValue, NodeTypeDefinition } from "./models"

/** Infer fields from declared defaults while retaining arbitrary JSON declarations. */
export function extensionDefinition(extension: Extension): NodeTypeDefinition {
  return {
    type: "custom",
    name: extension.displayName,
    description: `Extension: ${extension.name}`,
    inputTypes: [],
    settings: Object.entries(extension.adapter.settings ?? {}).map(([key, declaration]) => {
      const hasDefault = Object.hasOwn(declaration, "default")
      const value = declaration.default
      const json = !hasDefault || value === null || typeof value === "object"
      return {
        key, label: key,
        input: typeof value === "boolean" ? "checkbox" : typeof value === "number" ? "number" : "text",
        valueType: typeof value === "boolean" ? "boolean" : typeof value === "number" ? "number" : "string",
        required: Boolean(declaration.required && !hasDefault),
        ...(hasDefault ? { defaultValue: (json ? JSON.stringify(value) : value) as NodeSettingValue } : {}),
        ...(json ? { description: "Enter a JSON value, including quotes for strings." } : {}),
      }
    }),
  }
}

/** Convert form values to custom settings without inserting omitted optional declarations. */
export function extensionSettings(extension: Extension, values: Record<string, NodeSettingValue>): Record<string, unknown> {
  const settings: Record<string, unknown> = Object.create(null)
  for (const [key, declaration] of Object.entries(extension.adapter.settings ?? {})) {
    const hasDefault = Object.hasOwn(declaration, "default")
    const json = !hasDefault || declaration.default === null || typeof declaration.default === "object"
    const value = values[key]
    if (value === undefined || value === null || (json && value === "")) {
      if (hasDefault) settings[key] = declaration.default
      else if (declaration.required) throw new Error(`${key} is required`)
      continue
    }
    if (json) {
      try { settings[key] = JSON.parse(String(value)) } catch { throw new Error(`${key} must be valid JSON`) }
    } else settings[key] = value
  }
  return settings
}

/** Present existing custom JSON settings using the same controls as their declarations. */
export function extensionFormValues(extension: Extension, settings: Record<string, unknown>): Record<string, NodeSettingValue> {
  return Object.fromEntries(Object.entries(settings).map(([key, value]) => {
    const declaration = extension.adapter.settings?.[key]
    const json = !declaration || !Object.hasOwn(declaration, "default") || declaration.default === null || typeof declaration.default === "object"
    return [key, (json ? JSON.stringify(value) : value) as NodeSettingValue]
  }))
}
