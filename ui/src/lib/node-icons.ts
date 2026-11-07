import { AudioWaveform, Blocks, Languages, Mic, Monitor, type LucideIcon } from "lucide-react"

import type { NodeRole } from "@/stores/models"

/** Existing node icons shared by node views and registered extensions. */
export const nodeTypeIcons: Record<NodeRole, LucideIcon> = {
  audio_source: Mic,
  transcriber: AudioWaveform,
  translator: Languages,
  display: Monitor,
  custom: Blocks,
}
