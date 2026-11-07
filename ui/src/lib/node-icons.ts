import { AudioWaveform, Blocks, Image, Languages, Mic, Monitor, TextAlignStart, type LucideIcon } from "lucide-react"

import type { Extension, ExtensionDataType, Node, NodeRole } from "@/stores/models"

/** Existing node icons shared by node views and registered extensions. */
export const nodeTypeIcons: Record<NodeRole, LucideIcon> = {
  audio_source: Mic,
  transcriber: AudioWaveform,
  translator: Languages,
  display: Monitor,
  custom: Blocks,
}

/** Custom media types select icons without implying pipeline behavior. */
export const extensionTypeIcons: Record<ExtensionDataType, LucideIcon> = {
  image: Image,
  text: TextAlignStart,
  audio: AudioWaveform,
}

/** Prefer output, then input, then the generic extension icon when neither is declared. */
export function extensionIcon(adapter: Pick<Extension["adapter"], "inputType" | "outputType">): LucideIcon {
  const type = adapter.outputType ?? adapter.inputType
  return type ? extensionTypeIcons[type] : Blocks
}

/** Use known built-in icons or the custom extension's declared media types. */
export function nodeIcon(node: Node): LucideIcon {
  return node.type === "custom" ? extensionIcon(node) : nodeTypeIcons[node.type]
}
