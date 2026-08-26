export type NodeType = "audio_source" | "transcriber" | "translator" | "display"
export type NodeStatus = "connected" | "disconnected" | "degraded"
export type HostStatus = "online" | "offline" | "degraded"
export type ConnectionStatus = "connected" | "reconnecting" | "degraded"
export type DataType = "audio" | "transcription" | "translation"
export type ConnectionDirection = "incoming" | "outgoing"
export type ConnectionProtocol = "websocket" | "http" | "https"
export type JsonMetadata = Record<string, unknown>

export interface LoginRequest {
  username: string
  password: string
}

export interface User {
  id: string
  username: string
}

export interface LoginResponse {
  accessToken: string
  tokenType: "Bearer"
  expiresIn: number
  user: User
}

export interface Host {
  id: string
  name: string
  ip: string
  version: string
  status: HostStatus
  lastSeenAt: string
  metadata?: JsonMetadata
}

export interface HostList {
  items: Host[]
  total: number
}

export interface NodeConnection {
  id: string
  peerNodeId: string
  peerNodeType: NodeType
  peerName?: string
  direction: ConnectionDirection
  dataType: DataType
  status: ConnectionStatus
  protocol: ConnectionProtocol
  connectedAt: string
  lastActivityAt: string
  bytesTransferred?: number
  metadata?: JsonMetadata
}

export interface AudioSourceSettings {
  sampleRateHz: 8000 | 16000 | 22050 | 44100 | 48000
  channels: number
  encoding: "pcm_s16le" | "pcm_f32le" | "opus"
  deviceName?: string | null
}

export interface TranscriberSettings {
  model: string
  language: string
  task?: "transcribe" | "translate"
  beamSize?: number
  vadEnabled?: boolean
}

export interface TranslatorSettings {
  sourceLanguage: string
  targetLanguage: string
  provider: string
  model?: string | null
  preserveFormatting?: boolean
}

export interface DisplaySettings {
  maxLines: number
  fontScale: number
  showOriginal: boolean
  theme?: "light" | "dark" | "transparent"
}

interface NodeBase {
  id: string
  name: string
  description?: string | null
  enabled: boolean
  status: NodeStatus
  host: Host
  connections: NodeConnection[]
  createdAt: string
  updatedAt: string
}

export interface AudioSourceNode extends NodeBase {
  type: "audio_source"
  settings: AudioSourceSettings
}

export interface TranscriberNode extends NodeBase {
  type: "transcriber"
  settings: TranscriberSettings
}

export interface TranslatorNode extends NodeBase {
  type: "translator"
  settings: TranslatorSettings
}

export interface DisplayNode extends NodeBase {
  type: "display"
  settings: DisplaySettings
}

export type Node = AudioSourceNode | TranscriberNode | TranslatorNode | DisplayNode

interface NodeUpdateBase {
  name: string
  description?: string | null
  enabled: boolean
}

export type NodeUpdate =
  | (NodeUpdateBase & { type: "audio_source"; settings: AudioSourceSettings })
  | (NodeUpdateBase & { type: "transcriber"; settings: TranscriberSettings })
  | (NodeUpdateBase & { type: "translator"; settings: TranslatorSettings })
  | (NodeUpdateBase & { type: "display"; settings: DisplaySettings })

export interface NodeList {
  items: Node[]
  total: number
}

export interface Problem {
  type: string
  title: string
  status: number
  detail?: string
  instance?: string
  requestId?: string
}

export interface ValidationError {
  path: string
  message: string
}

export interface ValidationProblem extends Problem {
  errors: ValidationError[]
}
