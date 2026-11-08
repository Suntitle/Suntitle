import type { ExtensionDataType, Node, NodeRole, NodeTypeDefinition } from "./models"

/** Built-in producer media types; displays do not produce pipeline data. */
const outputTypes: Partial<Record<NodeRole, ExtensionDataType>> = {
  audio_source: "audio",
  transcriber: "text",
  translator: "text",
}

/** Preserve built-in role constraints and admit extensions with compatible output media. */
export function acceptsInput(definition: NodeTypeDefinition, node: Node): boolean {
  if (node.type !== "custom") return definition.inputTypes.includes(node.type)
  return node.outputType !== undefined && definition.inputTypes.some((role) => outputTypes[role] === node.outputType)
}
